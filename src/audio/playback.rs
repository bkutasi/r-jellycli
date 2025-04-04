use tracing::trace;
use async_trait::async_trait;
use std::sync::{Arc, Mutex as StdMutex}; // Keep std Mutex for AlsaPcmHandler
use tokio::sync::{broadcast, Mutex as TokioMutex};
// use rubato::{SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction}; // Removed - Unused import
use crate::audio::{
    alsa_handler::AlsaPcmHandler,
    alsa_writer::AlsaWriter,
    decoder::SymphoniaDecoder,
    error::AudioError,
    loop_runner::{PlaybackLoopExitReason, PlaybackLoopRunner},
    // processor::AudioProcessor, // Removed - Unused import
    progress::{SharedProgress},
    state_manager::{OnFinishCallback, PlaybackStateManager},
    stream_wrapper::ReqwestStreamWrapper,
};
use reqwest::Client;
use symphonia::core::io::{MediaSourceStream, MediaSourceStreamOptions};
use tokio::task::{self, JoinHandle};
use tracing::{debug, error, info, instrument, warn};

const LOG_TARGET: &str = "r_jellycli::audio::playback"; // Main orchestrator log target

/// Trait defining the controls for an audio playback backend.
#[async_trait]
pub trait AudioPlaybackControl: Send + Sync {
    /// Starts playing the audio stream from the given URL.
    /// Takes ownership of an `on_finish` callback to be executed when playback
    /// completes naturally (reaches end of stream).
    async fn play(
        &mut self,
        url: &str,
        total_duration_ticks: Option<i64>, // Keep for potential future use
        on_finish: OnFinishCallback,
        shutdown_rx: broadcast::Receiver<()>,
    ) -> Result<(), AudioError>;

    /// Pauses the current playback.
    async fn pause(&mut self) -> Result<(), AudioError>;

    /// Resumes the current playback.
    async fn resume(&mut self) -> Result<(), AudioError>;

    /// Gets the current playback position in ticks.
    async fn get_current_position_ticks(&self) -> Result<i64, AudioError>;

    /// Provides a way to pass the shared pause state Arc<TokioMutex<bool>>
    /// from the Player to the audio backend.
    fn set_pause_state_tracker(&mut self, state: Arc<TokioMutex<bool>>);

    /// Provides a way to pass the shared progress state Arc<TokioMutex<PlaybackProgressInfo>>
    /// from the Player to the audio backend.
    fn set_progress_tracker(&mut self, tracker: SharedProgress);

    /// Performs a full shutdown of the audio backend (e.g., closing ALSA device).
    /// Should be called before dropping the implementing struct.
    async fn shutdown(&mut self) -> Result<(), AudioError>;
}

/// Manages ALSA audio playback orchestration by coordinating dedicated components.
pub struct PlaybackOrchestrator {
    alsa_handler: Arc<StdMutex<AlsaPcmHandler>>, // The core sync handler
    alsa_writer: Option<Arc<AlsaWriter>>, // Async writer wrapper
    state_manager: Arc<TokioMutex<PlaybackStateManager>>, // State manager
    // Keep track of the playback loop task to ensure it finishes during shutdown
    playback_task_handle: Option<JoinHandle<Result<PlaybackLoopExitReason, AudioError>>>,
}

impl PlaybackOrchestrator {
    /// Creates a new playback orchestrator for the specified device.
    pub fn new(device_name: &str) -> Self {
        info!(target: LOG_TARGET, "Creating new PlaybackOrchestrator for device: {}", device_name);
        let alsa_handler = Arc::new(StdMutex::new(AlsaPcmHandler::new(device_name)));
        PlaybackOrchestrator {
            alsa_handler,
            alsa_writer: None, // Initialized during play
            state_manager: Arc::new(TokioMutex::new(PlaybackStateManager::new())),
            playback_task_handle: None,
        }
    }

    /// Internal method to set up components and spawn the playback loop task.
    #[instrument(skip(self, url, on_finish, shutdown_rx), fields(url))]
    async fn setup_and_run_playback_loop(
        &mut self,
        url: &str,
        _total_duration_ticks: Option<i64>, // Keep for potential future use
        on_finish: OnFinishCallback,
        shutdown_rx: broadcast::Receiver<()>,
    ) -> Result<(), AudioError> {
        info!(target: LOG_TARGET, "Setting up playback loop for URL: {}", url);

        // --- Ensure previous task is handled (optional, depends on desired behavior) ---
        if let Some(handle) = self.playback_task_handle.take() {
            warn!(target: LOG_TARGET, "Previous playback task was still running. Aborting it.");
            handle.abort(); // Abort previous task if play is called again
            // Consider awaiting briefly or logging if abort fails
        }

        // --- HTTP Streaming Setup ---
        let client = Client::new();
        let response = client.get(url).send().await?.error_for_status()?;
        debug!(target: LOG_TARGET, "HTTP Response Headers: {:#?}", response.headers());
        let stream = response.bytes_stream();
        let source = Box::new(ReqwestStreamWrapper::new_async(stream).await?);
        let mss = MediaSourceStream::new(source, MediaSourceStreamOptions::default());

        // --- Symphonia Decoder Setup ---
        let decoder = SymphoniaDecoder::new(mss)?;
        // let initial_spec = decoder.current_spec().ok_or(AudioError::InitializationError("Decoder failed to provide initial spec".to_string()))?; // Use current_spec
        // Spec is now determined *inside* the loop runner after the first frame decode.
        // let num_channels = initial_spec.channels.count(); // Determined inside loop runner

        // --- ALSA Initialization & Get Actual Rate ---
        // This logic is now moved inside PlaybackLoopRunner::run
        // let actual_rate = { ... };
        // debug!(target: LOG_TARGET, "Decoder rate: {}, ALSA actual rate: {}", initial_spec.rate, actual_rate);

        // --- Resampler Setup (Conditional) ---
        // This logic is now moved inside PlaybackLoopRunner::run
        // let decoder_rate = initial_spec.rate;
        // let resampler_instance: Option<Arc<TokioMutex<SincFixedIn<f32>>>> = if decoder_rate != actual_rate { ... };

        // --- Create Components ---
        let alsa_writer = Arc::new(AlsaWriter::new(Arc::clone(&self.alsa_handler)));
        self.alsa_writer = Some(Arc::clone(&alsa_writer));

        // let processor = Arc::new(AudioProcessor::new(resampler_instance, num_channels)); // Moved inside loop runner

        // Configure State Manager (already created in `new`, just configure it)
        {
            let mut state_manager_guard = self.state_manager.lock().await;
            state_manager_guard.set_on_finish_callback(on_finish);
            // Trackers should have been set via trait methods before calling play
            if state_manager_guard.get_pause_state_tracker().is_none() {
                 warn!(target: LOG_TARGET, "Play called before pause state tracker was set.");
                 // Optionally return error or proceed with default behavior
            }
        }

        // --- Create and Spawn Playback Loop Runner ---
        let loop_runner = PlaybackLoopRunner::new(
            decoder,
            // processor, // Removed - created inside runner
            Arc::clone(&self.alsa_handler), // Pass the handler instead
            alsa_writer,
            Arc::clone(&self.state_manager),
            shutdown_rx,
        );

        info!(target: LOG_TARGET, "Spawning playback loop task...");
        let state_manager_clone = Arc::clone(&self.state_manager);
        let alsa_writer_clone = Arc::clone(self.alsa_writer.as_ref().unwrap());

        let playback_task = task::spawn(async move {
            let loop_result = loop_runner.run().await;

            // --- Post-Loop Handling (inside the spawned task) ---
            match loop_result {
                Ok(PlaybackLoopExitReason::EndOfStream) => {
                    info!(target: LOG_TARGET, "Playback loop finished (EndOfStream). Draining ALSA...");
                    if let Err(e) = alsa_writer_clone.drain_async().await {
                        error!(target: LOG_TARGET, "Error draining ALSA buffer after EndOfStream: {}", e);
                        // Don't execute callback if drain fails? Or log and continue? Let's log and continue.
                    } else {
                        debug!(target: LOG_TARGET, "ALSA drain successful after EndOfStream.");
                    }
                    // Execute callback after successful drain (or if drain error is ignored)
                    state_manager_clone.lock().await.execute_on_finish_callback();
                    Ok(PlaybackLoopExitReason::EndOfStream)
                }
                Ok(PlaybackLoopExitReason::ShutdownSignal) => {
                    info!(target: LOG_TARGET, "Playback loop terminated by shutdown signal. Skipping final drain and finish callback.");
                    Ok(PlaybackLoopExitReason::ShutdownSignal)
                }
                Err(e) => {
                    error!(target: LOG_TARGET, "Playback loop failed with error: {}", e);
                    // Don't execute callback on error
                    Err(e)
                }
            }
        });

        self.playback_task_handle = Some(playback_task);

        Ok(())
    }
}

#[async_trait]
impl AudioPlaybackControl for PlaybackOrchestrator {
    #[instrument(skip(self, on_finish, shutdown_rx), fields(url))]
    async fn play(
        &mut self,
        url: &str,
        total_duration_ticks: Option<i64>,
        on_finish: OnFinishCallback,
        shutdown_rx: broadcast::Receiver<()>,
    ) -> Result<(), AudioError> {
        info!(target: LOG_TARGET, "AudioPlaybackControl::play called.");
        // 1. Set up the playback loop (spawns the inner task)
        self.setup_and_run_playback_loop(url, total_duration_ticks, on_finish, shutdown_rx).await?;

        // 2. Await the completion of the inner playback loop task
        if let Some(handle) = self.playback_task_handle.take() {
            info!(target: LOG_TARGET, "AudioPlaybackControl::play awaiting inner loop completion...");
            match handle.await {
                Ok(Ok(PlaybackLoopExitReason::EndOfStream)) => {
                    info!(target: LOG_TARGET, "Inner playback loop finished (EndOfStream).");
                    Ok(()) // Natural completion
                }
                Ok(Ok(PlaybackLoopExitReason::ShutdownSignal)) => {
                    info!(target: LOG_TARGET, "Inner playback loop finished (ShutdownSignal).");
                    Ok(()) // Explicit stop, not an error from play's perspective
                }
                Ok(Err(audio_err)) => {
                    error!(target: LOG_TARGET, "Inner playback loop failed: {}", audio_err);
                    Err(audio_err) // Propagate the audio error
                }
                Err(join_err) => {
                    error!(target: LOG_TARGET, "Failed to join inner playback loop task: {}", join_err);
                    // Convert JoinError into an AudioError
                    Err(AudioError::TaskJoinError(format!("Playback loop task join error: {}", join_err)))
                }
            }
        } else {
            // This case should ideally not happen if setup_and_run_playback_loop succeeded
            error!(target: LOG_TARGET, "Playback task handle was unexpectedly None after setup.");
            Err(AudioError::InvalidState("Playback task handle missing after setup".to_string()))
        }
    }

    #[instrument(skip(self))]
    async fn pause(&mut self) -> Result<(), AudioError> {
        info!(target: LOG_TARGET, "AudioPlaybackControl::pause called.");
        // Delegate directly to state manager
        self.state_manager.lock().await.pause().await // Delegate directly to state manager
    }

    #[instrument(skip(self))]
    async fn resume(&mut self) -> Result<(), AudioError> {
        info!(target: LOG_TARGET, "AudioPlaybackControl::resume called.");
        // Delegate directly to state manager
        self.state_manager.lock().await.resume().await // Delegate directly to state manager
    }

    #[instrument(skip(self))]
    async fn get_current_position_ticks(&self) -> Result<i64, AudioError> {
        trace!(target: LOG_TARGET, "AudioPlaybackControl::get_current_position_ticks called.");
        // Delegate directly to state manager
        self.state_manager.lock().await.get_current_position_ticks().await // Delegate directly to state manager
    }

    /// Sets the shared pause state tracker.
    fn set_pause_state_tracker(&mut self, state: Arc<TokioMutex<bool>>) {
        debug!(target: LOG_TARGET, "Pause state tracker configured via trait method.");
        // Block on the async lock - this method is sync
        // Consider making this async or using try_lock if blocking is undesirable
        futures::executor::block_on(async {
            self.state_manager.lock().await.set_pause_state_tracker(state);
        });
    }

    /// Sets the shared progress tracker.
    fn set_progress_tracker(&mut self, tracker: SharedProgress) {
        debug!(target: LOG_TARGET, "Progress tracker configured via trait method.");
        // Block on the async lock - this method is sync
        futures::executor::block_on(async {
            self.state_manager.lock().await.set_progress_tracker(tracker);
        });
    }

    /// Performs a full shutdown of the audio backend.
    #[instrument(skip(self))]
    async fn shutdown(&mut self) -> Result<(), AudioError> {
        info!(target: LOG_TARGET, "AudioPlaybackControl::shutdown called.");

        // 1. Abort the playback task if it's running
        if let Some(handle) = self.playback_task_handle.take() {
            info!(target: LOG_TARGET, "Aborting playback loop task...");
            handle.abort();
            // Optionally await the handle with a timeout to see if it finishes cleanly
            match tokio::time::timeout(std::time::Duration::from_secs(1), handle).await {
                Ok(Ok(Ok(reason))) => info!(target: LOG_TARGET, "Playback task finished after abort with reason: {:?}", reason),
                Ok(Ok(Err(e))) => warn!(target: LOG_TARGET, "Playback task finished with error after abort: {}", e),
                Ok(Err(join_err)) => warn!(target: LOG_TARGET, "Playback task join error after abort: {}", join_err),
                Err(_) => warn!(target: LOG_TARGET, "Playback task did not finish within timeout after abort."),
            }
        } else {
             debug!(target: LOG_TARGET, "No active playback task handle found during shutdown.");
        }

        // 2. Shutdown the ALSA writer (which handles the underlying ALSA handler)
        if let Some(writer) = self.alsa_writer.take() {
            info!(target: LOG_TARGET, "Shutting down ALSA writer...");
            if let Err(e) = writer.shutdown_async().await {
                error!(target: LOG_TARGET, "Error shutting down ALSA writer: {}", e);
                // Decide whether to return the error or just log it.
                // Let's return it for now, as ALSA cleanup is important.
                return Err(e);
            }
            info!(target: LOG_TARGET, "ALSA writer shut down successfully.");
        } else {
            debug!(target: LOG_TARGET, "No ALSA writer instance found during shutdown (likely play was never called).");
            // If play was never called, the underlying handler might still need closing if it was ever initialized.
            // However, the current logic initializes in play(). If we want shutdown capability
            // without play, the AlsaPcmHandler needs separate lifecycle management.
            // For now, assume shutdown only cleans up resources created during play.
        }

        // 3. Reset state manager progress (optional, might be useful)
        self.state_manager.lock().await.reset_progress_info().await;

        info!(target: LOG_TARGET, "PlaybackOrchestrator shutdown complete.");
        Ok(())
    }
}

impl Drop for PlaybackOrchestrator {
    fn drop(&mut self) {
        // IMPORTANT: Explicit async `shutdown()` is required for graceful cleanup.
        debug!(target: LOG_TARGET, "Dropping PlaybackOrchestrator. Explicit async shutdown() is required for graceful ALSA cleanup.");

        // Attempt a best-effort abort if shutdown wasn't called.
        if let Some(handle) = self.playback_task_handle.take() {
            warn!(target: LOG_TARGET, "PlaybackOrchestrator dropped without calling shutdown(). Aborting task.");
            handle.abort();
        }
        // Cannot call async shutdown here. Resources might leak if shutdown() wasn't called.
    }
}
