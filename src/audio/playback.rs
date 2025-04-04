// src/audio/playback.rs
use async_trait::async_trait; // Added this line
// Removed unused import: use futures::future::BoxFuture;
use std::sync::Mutex; // Use std Mutex for AlsaPcmHandler - Keep this one
use tokio::sync::Mutex as TokioMutex;
use symphonia::core::audio::Signal;
use rubato::{Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction};
use crate::audio::{
    alsa_handler::AlsaPcmHandler,
    // Import the new types from decoder
    decoder::{DecodeRefResult, DecodedBufferAndTimestamp, SymphoniaDecoder},
    error::AudioError,
    // format_converter, // Removed unused import
    progress::{PlaybackProgressInfo, SharedProgress}, // Removed PROGRESS_UPDATE_INTERVAL
    stream_wrapper::ReqwestStreamWrapper,
    sample_converter, // Added import
};
// use indicatif::{ProgressBar, ProgressStyle}; // Removed indicatif import
use tracing::{debug, error, info, trace, warn, instrument}; // Replaced log with tracing, added instrument
use reqwest::Client;
use std::sync::Arc;
use std::time::Instant;
// use symphonia::core::audio::SignalSpec; // Removed unused import
use symphonia::core::io::MediaSourceStream;
use symphonia::core::io::MediaSourceStreamOptions;
use symphonia::core::units::TimeBase;
use tokio::sync::broadcast;
use tokio::task;

// Import the specific sample type we'll decode into for now
use symphonia::core::sample::Sample;
use std::any::TypeId; // For checking generic type S


const LOG_TARGET: &str = "r_jellycli::audio::playback"; // Main orchestrator log target

/// Callback type for when playback finishes naturally.
type OnFinishCallback = Box<dyn FnOnce() + Send + Sync + 'static>;

/// Trait defining the controls for an audio playback backend.
#[async_trait]
pub trait AudioPlaybackControl: Send + Sync {
    /// Starts playing the audio stream from the given URL.
    /// Takes ownership of an `on_finish` callback to be executed when playback
    /// completes naturally (reaches end of stream).
    async fn play(
        &mut self,
        url: &str,
        total_duration_ticks: Option<i64>,
        on_finish: OnFinishCallback,
        shutdown_rx: broadcast::Receiver<()>, // Add shutdown receiver
    ) -> Result<(), AudioError>;

    /// Pauses the current playback.
    async fn pause(&mut self) -> Result<(), AudioError>;

    /// Resumes the current playback.
    async fn resume(&mut self) -> Result<(), AudioError>;

    // /// Stops playback completely and cleans up resources for the current stream.
    // async fn stop(&mut self) -> Result<(), AudioError>; // Removed - stop is handled by Player logic calling shutdown/dropping backend


    /// Gets the current playback position in ticks.
    async fn get_current_position_ticks(&self) -> Result<i64, AudioError>;

    // --- Volume/Mute/Seek methods removed ---

    // /// Sets a callback to be invoked when the current track finishes playing naturally.
    // /// Replaced by passing callback directly to `play`.
    // async fn set_on_finish_callback(&mut self, callback: OnFinishCallback);

    /// Provides a way to pass the shared pause state Arc<TokioMutex<bool>>
    /// from the Player to the audio backend. This is necessary because the pause
    /// state is managed externally by the Player based on commands, but the
    /// audio loop needs to react to it.
    fn set_pause_state_tracker(&mut self, state: Arc<TokioMutex<bool>>);

    /// Provides a way to pass the shared progress state Arc<TokioMutex<PlaybackProgressInfo>>
    /// from the Player to the audio backend.
    fn set_progress_tracker(&mut self, tracker: SharedProgress);

    /// Performs a full shutdown of the audio backend (e.g., closing ALSA device).
    /// Should be called before dropping the implementing struct.
    async fn shutdown(&mut self) -> Result<(), AudioError>;
}


/// Indicates the reason why the playback loop terminated successfully.
#[derive(Debug, PartialEq, Eq)]
enum PlaybackLoopExitReason {
    EndOfStream,
    ShutdownSignal,
}

/// Manages ALSA audio playback orchestration, using dedicated handlers for ALSA, decoding, etc.
pub struct PlaybackOrchestrator { // Renamed from AlsaPlayer
    // Wrap the handler in Arc<std::sync::Mutex>
    alsa_handler: Arc<Mutex<AlsaPcmHandler>>,
    // progress_bar: Option<Arc<ProgressBar>>, // Removed progress bar field
    progress_info: Option<SharedProgress>,
    resampler: Option<Arc<TokioMutex<SincFixedIn<f32>>>>,
    pause_state: Option<Arc<TokioMutex<bool>>>, // Added: Shared pause state
    on_finish_callback: Option<OnFinishCallback>, // Callback for when track finishes
}

impl PlaybackOrchestrator { // Renamed from AlsaPlayer
    /// Creates a new ALSA player instance for the specified device.
    pub fn new(device_name: &str) -> Self {
        info!(target: LOG_TARGET, "Creating new AlsaPlayer for device: {}", device_name);
        PlaybackOrchestrator { // Renamed from AlsaPlayer
            // Wrap the handler in Arc<std::sync::Mutex>
            alsa_handler: Arc::new(Mutex::new(AlsaPcmHandler::new(device_name))),
            // progress_bar: None, // Removed progress bar initialization
            progress_info: None,
            resampler: None,
            pause_state: None, // Initialize pause state
            on_finish_callback: None,
        }
    }

    // Note: These methods are now part of the AudioPlaybackControl trait implementation
    // /// Sets the shared progress tracker.
    // pub fn set_progress_tracker(&mut self, tracker: SharedProgress) {
    //     debug!(target: LOG_TARGET, "Progress tracker configured.");
    //     self.progress_info = Some(tracker);
    // }
    //
    // /// Sets the shared pause state tracker.
    // pub fn set_pause_state(&mut self, state: Arc<TokioMutex<bool>>) {
    //     debug!(target: LOG_TARGET, "Pause state tracker configured.");
    //     self.pause_state = Some(state);
    // }

    // --- Private Helper Methods ---
    /// Writes the decoded S16LE buffer to ALSA, handling blocking and shutdown signals.
    async fn _write_to_alsa(
        &self,
        s16_buffer: &[i16],
        num_channels: usize,
        shutdown_rx: &mut broadcast::Receiver<()>,
    ) -> Result<(), AudioError> {
        if s16_buffer.is_empty() || num_channels == 0 {
            return Ok(());
        }

        let total_frames = s16_buffer.len() / num_channels;
        let mut offset = 0;

        while offset < total_frames {
            // Check shutdown before potentially blocking
            if shutdown_rx.try_recv().is_ok() {
                info!(target: LOG_TARGET, "Shutdown signal received during ALSA write loop. Returning ShutdownRequested.");
                return Err(AudioError::ShutdownRequested); // Return specific error
            }

            let frames_remaining = total_frames - offset;
            // Determine a reasonable chunk size to send to spawn_blocking
            // This avoids moving huge buffers unnecessarily if writei handles smaller chunks well.
            // Let's try sending chunks related to typical ALSA period sizes, e.g., 1024 or 4096 frames.
            let chunk_frames = frames_remaining.min(4096); // Example chunk size
            let _chunk_samples = chunk_frames * num_channels; // Prefixed unused variable
            let buffer_chunk = s16_buffer[offset * num_channels .. (offset + chunk_frames) * num_channels].to_vec(); // Copy chunk to move

            // Clone the Arc containing the std::sync::Mutex
            let handler_clone = Arc::clone(&self.alsa_handler); // Correct: Pass reference to Arc

            // Perform the blocking ALSA write in spawn_blocking
            trace!(target: LOG_TARGET, "Calling alsa_handler.write_s16_buffer with {} frames in blocking task...", chunk_frames);
            let write_result = task::spawn_blocking(move || {
                // Lock the std::sync::Mutex synchronously inside the blocking thread
                match handler_clone.lock() {
                    Ok(handler_guard) => handler_guard.write_s16_buffer(&buffer_chunk), // Pass copied chunk
                    Err(poisoned) => {
                        error!(target: LOG_TARGET, "ALSA handler mutex poisoned: {}", poisoned);
                        Err(AudioError::InvalidState("ALSA handler mutex poisoned".to_string()))
                    }
                }
            }).await?; // Await the JoinHandle, propagate JoinError if task panics

            trace!(target: LOG_TARGET, "alsa_handler.write_s16_buffer result: {:?}", write_result.as_ref().map_err(|e| format!("{:?}", e)));
            match write_result {
                 Ok(0) => { // Recovered underrun signaled by write_s16_buffer returning Ok(0)
                     warn!(target: LOG_TARGET, "ALSA underrun recovered, retrying write for the same chunk.");
                     // Don't advance offset, retry the same chunk.
                     // Add a small sleep to avoid busy-looping if ALSA isn't ready immediately.
                     tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                     continue; // Continue the while loop to retry the chunk
                 }
                 // Removed extra closing brace that was here
                 Ok(frames_written) if frames_written > 0 => {
                     // Note: write_s16_buffer returns frames written for the chunk
                     let actual_frames_written = frames_written.min(chunk_frames); // Ensure we don't exceed chunk size
                     offset += actual_frames_written;
                     trace!(target: LOG_TARGET, "Wrote {} frames to ALSA (total {}/{})", actual_frames_written, offset, total_frames);
                 }
                 Ok(_) => { // Should not happen if 0 means recovered underrun
                     trace!(target: LOG_TARGET, "ALSA write returned 0 frames unexpectedly, yielding.");
                     // Yield to allow ALSA buffer to drain if it returned 0 without error/recovery
                     task::yield_now().await;
                 }
                 Err(e @ AudioError::AlsaError(_)) => {
                     error!(target: LOG_TARGET, "Unrecoverable ALSA write error: {}", e);
                     return Err(e);
                 }
                  Err(e) => {
                      error!(target: LOG_TARGET, "Unexpected error during ALSA write: {}", e);
                      return Err(e);
                  }
            }
        }
        Ok(())
    }

    // Conversion functions moved to sample_converter module


    /// Updates the shared progress information based on the current timestamp.
    async fn _update_progress(
        &self,
        current_ts: u64,
        track_time_base: Option<TimeBase>,
        last_progress_update_time: &mut Instant,
    ) {
        // Update progress roughly every second
        if last_progress_update_time.elapsed() < std::time::Duration::from_secs(1) {
            return;
        }

        if let (Some(progress_arc), Some(time_base)) = (&self.progress_info, track_time_base) {
            let current_seconds = time_base.calc_time(current_ts).seconds as f64
                + time_base.calc_time(current_ts).frac;

            trace!(target: LOG_TARGET, "Updating progress: TS={}, Seconds={:.2}", current_ts, current_seconds);

            match progress_arc.lock().await {
                // Removed Ok wrapping since Tokio MutexGuard isn't Result
                mut progress_guard => {
                    progress_guard.current_seconds = current_seconds;
                    *last_progress_update_time = Instant::now();
                }
                // Removed Err arm for PoisonError, Tokio Mutex panics on poison
            }
        } else {
            if self.progress_info.is_none() {
                trace!(target: LOG_TARGET, "Skipping progress update: progress_info not set.");
            }
            if track_time_base.is_none() {
                trace!(target: LOG_TARGET, "Skipping progress update: track_time_base not set.");
            }
        }
    }


    /// The main loop for decoding packets and sending them to ALSA.
    async fn playback_loop( // Removed generic type <S>, changed return type
        &mut self,
        mut decoder: SymphoniaDecoder,
        // pb: Arc<ProgressBar>, // Removed pb parameter
        mut shutdown_rx: broadcast::Receiver<()>,
    ) -> Result<PlaybackLoopExitReason, AudioError> { // Changed return type
        info!(target: LOG_TARGET, "Starting playback loop.");
        let mut last_progress_update_time = Instant::now();
        let track_time_base = decoder.time_base();
        let mut was_paused = false; // Track previous pause state for transitions

        loop {
            // --- Pause Check ---
            // --- Pause Check and ALSA State Transition ---
            if let Some(pause_state_arc) = &self.pause_state {
                let is_paused = *pause_state_arc.lock().await;

                    trace!(target: LOG_TARGET, "Pause check: is_paused = {}, was_paused = {}", is_paused, was_paused);
                // --- Handle State Transitions ---
                if is_paused && !was_paused {
                    // Transition: Playing -> Paused
                    debug!(target: LOG_TARGET, "Pause state changed: Playing -> Paused. Requesting ALSA pause.");
                    let handler_clone = Arc::clone(&self.alsa_handler);
                    trace!(target: LOG_TARGET, "Attempting to call alsa_handler.pause() in blocking task...");
                    let pause_result = task::spawn_blocking(move || {
                        match handler_clone.lock() {
                            Ok(handler_guard) => handler_guard.pause(),
                            Err(poisoned) => {
                                error!(target: LOG_TARGET, "ALSA handler mutex poisoned during pause attempt: {}", poisoned);
                                Err(AudioError::InvalidState("ALSA handler mutex poisoned".to_string()))
                            }
                        }
                    }).await;
                    trace!(target: LOG_TARGET, "ALSA pause task completed. Result: {:?}", pause_result.as_ref().map(|r| r.is_ok())); // Log before consuming
                    if let Err(e) = pause_result.map_err(|je| AudioError::TaskJoinError(je.to_string())).and_then(|res| res) {
                        warn!(target: LOG_TARGET, "Failed to pause ALSA device: {}", e);
                        // Continue playback loop, but log the error. Might recover later.
                    }
                } else if !is_paused && was_paused {
                    // Transition: Paused -> Playing
                    debug!(target: LOG_TARGET, "Pause state changed: Paused -> Playing. Requesting ALSA resume.");
                    let handler_clone = Arc::clone(&self.alsa_handler);
                    trace!(target: LOG_TARGET, "Attempting to call alsa_handler.resume() in blocking task...");
                    let resume_result = task::spawn_blocking(move || {
                         match handler_clone.lock() {
                            Ok(handler_guard) => {
                                trace!(target: LOG_TARGET, "Inside blocking task: Calling handler_guard.resume()...");
                                let res = handler_guard.resume();
                                trace!(target: LOG_TARGET, "Inside blocking task: handler_guard.resume() returned: {:?}", res.as_ref().map_err(|e| format!("{:?}", e)));
                                res
                            }
                            Err(poisoned) => {
                                error!(target: LOG_TARGET, "ALSA handler mutex poisoned during resume attempt: {}", poisoned);
                                Err(AudioError::InvalidState("ALSA handler mutex poisoned".to_string()))
                            }
                        }
                    }).await;
                    trace!(target: LOG_TARGET, "ALSA resume task completed. Result: {:?}", resume_result.as_ref().map(|r| r.is_ok())); // Log before consuming
                     if let Err(e) = resume_result.map_err(|je| AudioError::TaskJoinError(je.to_string())).and_then(|res| res) {
                        warn!(target: LOG_TARGET, "Failed to resume ALSA device: {}", e);
                        // Continue playback loop, but log the error.
                    }
                }
                was_paused = is_paused; // Update state *after* handling transition

                // --- Wait While Paused ---
                if is_paused {
                    loop { // Inner loop to wait while paused *after* ALSA pause command sent
                        trace!(target: LOG_TARGET, "Entering wait loop (is_paused = true)");
                        trace!(target: LOG_TARGET, "Playback paused (ALSA pause requested), waiting...");
                        // Re-check pause state and shutdown signal
                        let current_pause_state = *pause_state_arc.lock().await;
                        if !current_pause_state {
                            debug!(target: LOG_TARGET, "Pause state changed: Paused -> Playing (detected in wait loop). Requesting ALSA resume.");
                            // --- Call Resume Logic Directly ---
                            let handler_clone = Arc::clone(&self.alsa_handler);
                            trace!(target: LOG_TARGET, "Attempting to call alsa_handler.resume() in blocking task (from wait loop)...");
                            let resume_result = task::spawn_blocking(move || {
                                 match handler_clone.lock() {
                                    Ok(handler_guard) => {
                                        trace!(target: LOG_TARGET, "Inside blocking task: Calling handler_guard.resume()...");
                                        let res = handler_guard.resume();
                                        trace!(target: LOG_TARGET, "Inside blocking task: handler_guard.resume() returned: {:?}", res.as_ref().map_err(|e| format!("{:?}", e)));
                                        res
                                    }
                                    Err(poisoned) => {
                                        error!(target: LOG_TARGET, "ALSA handler mutex poisoned during resume attempt: {}", poisoned);
                                        Err(AudioError::InvalidState("ALSA handler mutex poisoned".to_string()))
                                    }
                                }
                            }).await;
                            trace!(target: LOG_TARGET, "ALSA resume task completed (from wait loop). Result: {:?}", resume_result.as_ref().map(|r| r.is_ok()));
                             if let Err(e) = resume_result.map_err(|je| AudioError::TaskJoinError(je.to_string())).and_then(|res| res) {
                                warn!(target: LOG_TARGET, "Failed to resume ALSA device (from wait loop): {}", e);
                                // Continue playback loop, but log the error.
                            }
                            // --- End Resume Logic ---
                            was_paused = false; // Update was_paused immediately since we handled the transition
                            trace!(target: LOG_TARGET, "Exiting wait loop after handling resume.");
                            break; // Break inner wait loop
                        }

                        tokio::select! {
                            biased; // Prioritize shutdown check
                            _ = shutdown_rx.recv() => {
                                info!(target: LOG_TARGET, "Shutdown signal received while paused.");
                                return Ok(PlaybackLoopExitReason::ShutdownSignal);
                            }
                            _ = tokio::time::sleep(std::time::Duration::from_millis(100)) => {
                                // Continue the inner pause-checking loop
                            }
                        }
                    }
                }
            }
            // --- End Pause Check ---

            trace!(target: LOG_TARGET, "--- Playback loop iteration start (after pause check) ---");
            // --- Decode Next Frame (Owned) ---
            let decode_result = decoder.decode_next_frame_owned(&mut shutdown_rx).await;
            // Adjust trace log for new enum structure
            trace!(target: LOG_TARGET, "Decoder result: {:?}", decode_result.as_ref().map(|r| match r { DecodeRefResult::DecodedOwned(buf_ts) => format!("DecodedOwned(type={:?}, ts={})", buf_ts.type_id(), buf_ts.timestamp()), DecodeRefResult::EndOfStream => "EndOfStream".to_string(), DecodeRefResult::Skipped(s) => format!("Skipped({})", s), DecodeRefResult::Shutdown => "Shutdown".to_string() }).map_err(|e| format!("{:?}", e)));

            match decode_result {
                Ok(DecodeRefResult::DecodedOwned(decoded_buffer_ts)) => {
                    // Extract buffer and timestamp based on the enum variant
                    // Prefix unused `ts` with underscore in match arms
                    let (num_channels, _current_ts, s16_vec) = match decoded_buffer_ts { // Renamed _current_ts to current_ts
                        DecodedBufferAndTimestamp::U8(audio_buffer, ts) => {
                            let nc = audio_buffer.spec().channels.count();
                            let vec = self._process_buffer(audio_buffer, ts, /* &pb, */ track_time_base, &mut last_progress_update_time).await?; // Removed pb arg
                            (nc, ts, vec)
                        }
                        DecodedBufferAndTimestamp::S16(audio_buffer, ts) => {
                            let nc = audio_buffer.spec().channels.count();
                            let vec = self._process_buffer(audio_buffer, ts, /* &pb, */ track_time_base, &mut last_progress_update_time).await?; // Removed pb arg
                            (nc, ts, vec)
                        }
                        DecodedBufferAndTimestamp::S24(audio_buffer, ts) => {
                            let nc = audio_buffer.spec().channels.count();
                            let vec = self._process_buffer(audio_buffer, ts, /* &pb, */ track_time_base, &mut last_progress_update_time).await?; // Removed pb arg
                            (nc, ts, vec)
                        }
                        DecodedBufferAndTimestamp::S32(audio_buffer, ts) => {
                            let nc = audio_buffer.spec().channels.count();
                            let vec = self._process_buffer(audio_buffer, ts, /* &pb, */ track_time_base, &mut last_progress_update_time).await?; // Removed pb arg
                            (nc, ts, vec)
                        }
                        DecodedBufferAndTimestamp::F32(audio_buffer, ts) => {
                            let nc = audio_buffer.spec().channels.count();
                            let vec = self._process_buffer(audio_buffer, ts, /* &pb, */ track_time_base, &mut last_progress_update_time).await?; // Removed pb arg
                            (nc, ts, vec)
                        }
                        DecodedBufferAndTimestamp::F64(audio_buffer, ts) => {
                            let nc = audio_buffer.spec().channels.count();
                            let vec = self._process_buffer(audio_buffer, ts, /* &pb, */ track_time_base, &mut last_progress_update_time).await?; // Removed pb arg
                            (nc, ts, vec)
                        }
                    };

                    // If processing returned None (e.g., conversion/resampling error), skip to next iteration
                    let s16_vec = match s16_vec {
                        Some(vec) => vec,
                        None => continue, // Skip this iteration if processing failed
                    };
                    // --- Check if buffer is empty after potential resampling/conversion ---
                    if s16_vec.is_empty() {
                        trace!(target: LOG_TARGET, "Skipping empty buffer after conversion/resampling.");
                        continue;
                    }

                    // --- ALSA Playback ---
                    // Use the original num_channels, as resampling preserves channel count
                    trace!(target: LOG_TARGET, "Calling _write_to_alsa with {} interleaved frames...", s16_vec.len() / num_channels);
                    if let Err(e) = self._write_to_alsa(&s16_vec, num_channels, &mut shutdown_rx).await {
                        // Handle ShutdownRequested specifically
                        if matches!(e, AudioError::ShutdownRequested) {
                            info!(target: LOG_TARGET, "Shutdown requested during ALSA write, exiting playback loop.");
                            return Ok(PlaybackLoopExitReason::ShutdownSignal);
                        }
                        // Handle other errors
                        error!(target: LOG_TARGET, "ALSA Write Error: {}", e);
                        // pb.abandon_with_message(format!("ALSA Write Error: {}", e)); // Removed pb call
                        return Err(e); // Propagate other errors
                    }
                }
                Ok(DecodeRefResult::Skipped(reason)) => {
                     warn!(target: LOG_TARGET, "Decoder skipped packet: {}", reason);
                     continue;
                }
                Ok(DecodeRefResult::EndOfStream) => {
                    info!(target: LOG_TARGET, "Decoder reached end of stream. Flushing resampler if necessary...");

                    // --- Flush Resampler ---
                    if let Some(resampler_arc) = self.resampler.as_ref() {
                        let mut resampler = resampler_arc.lock().await;
                        trace!(target: LOG_TARGET, "Calling resampler.process_last()...");
                        // Try calling process_last directly on the guard via DerefMut coercion
                        // Call process_last on the resampler itself (dereferencing the guard)
                        // Explicitly get mutable reference and use UFCS for trait method call
                        // Get mutable reference before the match
                        // Get mutable reference for the match expression below
                       let resampler_instance = &mut *resampler;
                       // Flush the resampler by processing empty input slices for each channel
                       let num_channels = resampler_instance.nbr_channels();
                       let empty_inputs: Vec<&[f32]> = vec![&[]; num_channels];
                       match resampler_instance.process(&empty_inputs, None) {
                            Ok(f32_output_vecs) => {
                                if !f32_output_vecs.is_empty() && !f32_output_vecs[0].is_empty() {
                                    trace!(target: LOG_TARGET, "Resampler flush successful, got {} output frames.", f32_output_vecs.get(0).map_or(0, |v| v.len()));
                                    match sample_converter::convert_f32_vecs_to_s16(f32_output_vecs) { // Use sample_converter module
                                        Ok(s16_vec) => {
                                            if !s16_vec.is_empty() {
                                                // Use the getter for requested_spec
                                                // Use the getter for requested_spec
                                                let num_channels = self.alsa_handler.lock().unwrap().get_requested_spec().map_or(2, |s| s.channels.count()); // Get channels safely using getter
                                                trace!(target: LOG_TARGET, "Writing flushed resampler buffer ({} frames) to ALSA...", s16_vec.len() / num_channels);
                                                if let Err(e) = self._write_to_alsa(&s16_vec, num_channels, &mut shutdown_rx).await {
                                                    error!(target: LOG_TARGET, "Error writing flushed buffer to ALSA: {}", e);
                                                    // Decide how to handle: return error or just log? Let's return error.
                                                    return Err(e);
                                                }
                                            } else {
                                                trace!(target: LOG_TARGET, "Flushed resampler buffer converted to empty S16 buffer, skipping write.");
                                            }
                                        }
                                        Err(e) => {
                                            error!(target: LOG_TARGET, "Failed to convert flushed resampler buffer to S16: {}", e);
                                            // Decide how to handle: return error or just log? Let's return error.
                                            return Err(e);
                                        }
                                    }
                                } else {
                                    trace!(target: LOG_TARGET, "Resampler flush returned no frames.");
                                }
                            }
                            Err(e) => {
                                error!(target: LOG_TARGET, "Resampler flush (process_last) failed: {}", e);
                                // Decide how to handle: return error or just log? Let's return error.
                                // Use the correct error variant
                                return Err(AudioError::ResamplingError(format!("Resampler flush failed: {}", e)));
                            }
                        }
                    } else {
                        trace!(target: LOG_TARGET, "No resampler active, no flush needed.");
                    }

                    // --- Proceed with normal EndOfStream after flushing ---
                    trace!(target: LOG_TARGET, "Finished flushing (if applicable). Breaking playback loop due to EndOfStream.");
                    return Ok(PlaybackLoopExitReason::EndOfStream); // Return reason
                }
                 Ok(DecodeRefResult::Shutdown) => {
                    info!(target: LOG_TARGET, "Decoder received shutdown signal.");
                    // pb.abandon_with_message("Playback stopped"); // Removed pb call
                    return Ok(PlaybackLoopExitReason::ShutdownSignal); // Return reason
                }
                Err(e) => {
                    error!(target: LOG_TARGET, "Fatal decoder error: {}", e);
                    // pb.abandon_with_message(format!("Decoder Error: {}", e)); // Removed pb call
                    return Err(e);
                }
            }
        } // end 'decode_loop

        // --- Post-Loop Cleanup (Drain removed, handled by caller based on exit reason) ---
        // The loop now handles all exit conditions (EndOfStream, ShutdownSignal, Error)
        // via return statements, so code execution should not reach here.
    }

    // --- Public Methods ---

    /// Internal method to stream, decode, and play. Called by the `play` trait method.
    /// Takes ownership of the finish callback.
    #[instrument(skip(self, shutdown_rx, on_finish), fields(url))]
    async fn internal_stream_decode_and_play(
        &mut self,
        url: &str,
        _total_duration_ticks: Option<i64>, // Keep for potential future use (e.g., progress bar)
        shutdown_rx: broadcast::Receiver<()>, // Removed mut
        on_finish: OnFinishCallback,
    ) -> Result<(), AudioError> {
        // Store the callback
        self.on_finish_callback = Some(on_finish);

        info!(target: LOG_TARGET, "Starting stream/decode/play for URL: {}", url);

        // --- HTTP Streaming Setup ---
        let client = Client::new();
        let response = client.get(url).send().await?.error_for_status()?;
        debug!(target: LOG_TARGET, "HTTP Response Headers: {:#?}", response.headers());
        let content_length = response.content_length();
        debug!(target: LOG_TARGET, "HTTP response received. Content-Length: {:?}", content_length);
        let stream = response.bytes_stream();
        let source = Box::new(ReqwestStreamWrapper::new_async(stream).await?);
        let mss = MediaSourceStream::new(source, MediaSourceStreamOptions { buffer_len: 64 * 1024 }); // Use default buffer size (64KB)
        // --- Symphonia Decoder Setup ---
        let decoder = SymphoniaDecoder::new(mss)?; // Removed mut
        let initial_spec = decoder.initial_spec().ok_or(AudioError::InitializationError("Decoder failed to provide initial spec".to_string()))?;
        let _track_time_base = decoder.time_base();
// --- ALSA Initialization & Get Actual Rate ---
// --- ALSA Initialization & Get Actual Rate ---
let actual_rate = {
   let mut handler_guard = self.alsa_handler.lock().map_err(|e| AudioError::InvalidState(format!("ALSA handler mutex poisoned on init: {}", e)))?;
   handler_guard.initialize(initial_spec)?;
   handler_guard.get_actual_rate().ok_or_else(|| AudioError::InitializationError("ALSA handler did not return actual rate after initialization".to_string()))?
};
debug!(target: LOG_TARGET, "Decoder rate: {}, ALSA actual rate: {}", initial_spec.rate, actual_rate);

// --- Resampler Setup (Conditional) ---
let decoder_rate = initial_spec.rate;
if decoder_rate != actual_rate {
    info!(target: LOG_TARGET, "Sample rate mismatch (Decoder: {}, ALSA: {}). Initializing resampler.", decoder_rate, actual_rate);
    let params = SincInterpolationParameters {
        sinc_len: 256, // Quality parameter, higher is better but slower
        f_cutoff: 0.95, // Cutoff frequency relative to Nyquist
        interpolation: SincInterpolationType::Linear, // Faster interpolation
        oversampling_factor: 256, // Higher means better quality
        window: WindowFunction::BlackmanHarris2, // Good quality window function
    };
    let chunk_size = 512; // Process in chunks (aligned with ALSA period)
    let resampler = SincFixedIn::<f32>::new(
        actual_rate as f64 / decoder_rate as f64, // ratio = target_rate / source_rate
        2.0, // max_resample_ratio_difference - allow some flexibility
        params,
        chunk_size,
        initial_spec.channels.count(), // Number of channels
    ).map_err(|e| AudioError::InitializationError(format!("Failed to create resampler: {}", e)))?;
    self.resampler = Some(Arc::new(TokioMutex::new(resampler)));
} else {
    info!(target: LOG_TARGET, "Sample rates match ({} Hz). Resampling disabled.", actual_rate);
    self.resampler = None;
}

        // --- Progress Bar & Info Setup ---
        // let pb = self._setup_progress(content_length, track_time_base, Some(initial_spec), total_duration_ticks).await?; // Removed progress setup

        // --- Call Non-Generic Playback Loop ---
        // The loop now handles different buffer types internally.
        info!(target: LOG_TARGET, "Starting playback loop (handles format internally)...");
        // --- Call Non-Generic Playback Loop ---
        // The loop now handles different buffer types internally.
        info!(target: LOG_TARGET, "Starting playback loop (handles format internally)...");
        let loop_result = self.playback_loop(decoder, /* pb, */ shutdown_rx).await; // Removed pb arg

        // --- End Playback Loop ---
        let _ = match loop_result { // Ignore the result as suggested by warning
            Ok(PlaybackLoopExitReason::EndOfStream) => {
                info!(target: LOG_TARGET, "Playback loop finished normally (EndOfStream). Draining ALSA buffer...");
                // Lock the mutex before calling drain
                // Lock the mutex before calling drain
                if let Ok(guard) = self.alsa_handler.lock() {
                    if let Err(e) = guard.drain() {
                        // Log error but don't necessarily fail the whole operation,
                        // as playback itself completed.
                        error!(target: LOG_TARGET, "Error draining ALSA buffer after EndOfStream: {}", e);
                    } else {
                        debug!(target: LOG_TARGET, "ALSA drain successful after EndOfStream.");
                    }
                } else {
                    error!(target: LOG_TARGET, "Failed to lock ALSA handler mutex during post-EOF drain.");
                }
                Ok(()) // Playback completed successfully overall
            }
            Ok(PlaybackLoopExitReason::ShutdownSignal) => {
                info!(target: LOG_TARGET, "Playback loop terminated by shutdown signal. Skipping final drain and finish callback.");
                Ok(()) // Shutdown is not an error state for the playback function itself
            }
            Err(ref e) => {
                error!(target: LOG_TARGET, "Playback loop failed with error: {}", e);
                Err(e) // Propagate the error
            }
        };

        // --- Handle Finish Callback ---
        // Call the callback only if the loop exited normally (EndOfStream)
        // --- Handle Finish Callback ---
        // Call the callback only if the loop exited normally (EndOfStream)
        if matches!(loop_result, Ok(PlaybackLoopExitReason::EndOfStream)) {
             if let Some(callback) = self.on_finish_callback.take() {
                 info!(target: LOG_TARGET, "Executing on_finish callback.");
                 callback();
             } else {
                 warn!(target: LOG_TARGET, "Playback finished but no on_finish callback was set or it was already taken.");
             }
             // Return Ok(()) to indicate successful completion of playback.
             Ok(())
        } else {
            // If loop exited due to shutdown or error, propagate the result without calling callback.
            loop_result.map(|_| ()) // Discard the Ok(ShutdownSignal) value, keep Err(e)
        }
    }



    /// Performs graceful asynchronous shutdown of the playback orchestrator.
    /// This should be called explicitly before dropping the orchestrator to ensure
    /// potentially blocking cleanup operations (like ALSA drain/close) complete.
    #[instrument(skip(self))]
    pub async fn shutdown(&mut self) -> Result<(), AudioError> {
        info!(target: LOG_TARGET, "Shutting down PlaybackOrchestrator asynchronously.");

        // 1. Progress Bar cleanup removed

        // 2. Reset Progress Info (Async lock)
        if let Some(progress_mutex) = self.progress_info.take() { // Take ownership
            debug!(target: LOG_TARGET, "Resetting shared progress info...");
            let mut info = progress_mutex.lock().await; // Async lock
            *info = PlaybackProgressInfo::default();
            debug!(target: LOG_TARGET, "Reset shared progress info.");
            // Lock guard drops automatically here
        }
        // 3. Close ALSA Handler (Using spawn_blocking for the potentially blocking close)
        debug!(target: LOG_TARGET, "Attempting to shut down ALSA handler in blocking task...");
        let handler_clone = Arc::clone(&self.alsa_handler); // Clone Arc for the blocking task
        let shutdown_result = task::spawn_blocking(move || {
            match handler_clone.lock() { // std::sync::Mutex lock inside blocking task
                Ok(mut guard) => {
                    // Call shutdown_device, which triggers close_internal -> pcm.take() -> drop -> snd_pcm_close
                    guard.shutdown_device();
                    debug!(target: LOG_TARGET, "ALSA handler shutdown_device() called successfully within blocking task.");
                    Ok(()) // Indicate success from the blocking task's perspective
                }
                Err(poisoned) => {
                    error!(target: LOG_TARGET, "ALSA handler mutex poisoned during blocking shutdown: {}", poisoned);
                    // Return an error that can be handled after awaiting the task
                    Err(AudioError::InvalidState("ALSA handler mutex poisoned during shutdown".to_string()))
                }
            }
        }).await; // Await the JoinHandle

        // Handle the result from spawn_blocking
        match shutdown_result {
            Ok(Ok(())) => {
                debug!(target: LOG_TARGET, "ALSA handler shutdown task completed successfully.");
            }
            Ok(Err(e)) => {
                error!(target: LOG_TARGET, "ALSA handler shutdown task returned error: {}", e);
                // Propagate the error from the blocking task
                return Err(e);
            }
            Err(join_err) => {
                error!(target: LOG_TARGET, "ALSA handler shutdown task panicked or was cancelled: {}", join_err);
                // Return a specific error for join errors
                return Err(AudioError::TaskJoinError(format!("ALSA shutdown task failed: {}", join_err)));
            }
        }

        // 4. Clear Resampler (Non-blocking)
        self.resampler = None;
        debug!(target: LOG_TARGET, "Cleared resampler.");


        info!(target: LOG_TARGET, "PlaybackOrchestrator shutdown complete.");
        Ok(())
    }


    // Helper function moved inside impl block
    async fn _process_buffer<S: Sample + std::fmt::Debug + Send + Sync + 'static>(
        &mut self,
        audio_buffer: symphonia::core::audio::AudioBuffer<S>,
        current_ts: u64,
        // pb: &ProgressBar, // Removed pb parameter
        track_time_base: Option<TimeBase>, // Renamed _track_time_base
        last_progress_update_time: &mut Instant, // Renamed _last_progress_update_time
    ) -> Result<Option<Vec<i16>>, AudioError> { // Return Option<Vec<i16>>
        trace!(target: LOG_TARGET, "Processing buffer: {} frames, ts={}", audio_buffer.frames(), current_ts);

        // --- Progress Update ---
        self._update_progress(current_ts, track_time_base, last_progress_update_time).await; // Uncommented and removed pb arg

        let s16_vec: Vec<i16>;

        // --- Resampling Logic ---
        if let Some(resampler_arc) = self.resampler.as_ref() {
            let mut resampler = resampler_arc.lock().await;
            trace!(target: LOG_TARGET, "Resampling buffer in chunks...");

            // 1. Convert the entire input buffer to f32 vectors first
            let f32_input_vecs = match sample_converter::convert_buffer_to_f32_vecs(audio_buffer) {
                Ok(vecs) => vecs,
                Err(e) => {
                    warn!(target: LOG_TARGET, "Failed to convert buffer to F32 for resampling: {}. Skipping buffer.", e);
                    return Ok(None); // Indicate skip
                }
            };

            if f32_input_vecs.is_empty() || f32_input_vecs[0].is_empty() {
                trace!(target: LOG_TARGET, "Input buffer is empty after F32 conversion, skipping resampling.");
                return Ok(None);
            }

            let num_channels = f32_input_vecs.len();
            let total_input_frames = f32_input_vecs[0].len();
            let mut processed_frames = 0;
            let mut accumulated_output_vecs: Vec<Vec<f32>> = vec![Vec::new(); num_channels]; // Initialize output accumulator

            // 2. Process the f32 input vectors in fixed chunks matching the resampler's internal chunk size
            let needed_input_frames = resampler.input_frames_next(); // Use input_frames_next() again
            while processed_frames < total_input_frames {
                let remaining_frames = total_input_frames - processed_frames;
                let current_chunk_size = remaining_frames.min(needed_input_frames);

                // If the chunk size is 0, break the loop
                if current_chunk_size == 0 {
                    trace!(target: LOG_TARGET, "No more input frames to process.");
                    break;
                }

                let end_frame = processed_frames + current_chunk_size;

                // Create the input chunk for the resampler
                let mut input_chunk: Vec<&[f32]> = Vec::with_capacity(num_channels);
                for ch in 0..num_channels {
                    // Ensure we don't slice beyond the bounds of the input vector
                    if processed_frames < f32_input_vecs[ch].len() && end_frame <= f32_input_vecs[ch].len() {
                        input_chunk.push(&f32_input_vecs[ch][processed_frames..end_frame]);
                    } else {
                        // This case should ideally not happen if conversion is correct, but handle defensively
                        error!(target: LOG_TARGET, "Inconsistent input vector length detected during chunking at channel {}, frame {}. Input len: {}, end_frame: {}", ch, processed_frames, f32_input_vecs[ch].len(), end_frame);
                        // Push an empty slice to avoid panic, error will likely occur in resampler.process
                        input_chunk.push(&[]);
                    }
                }


                trace!(target: LOG_TARGET, "Processing chunk: frames {}..{} (size {})", processed_frames, end_frame - 1, current_chunk_size);

                // Process the chunk
                match resampler.process(&input_chunk, None) {
                    Ok(output_chunk) => {
                        if !output_chunk.is_empty() && !output_chunk[0].is_empty() {
                            trace!(target: LOG_TARGET, "Resampler output chunk size: {} frames", output_chunk[0].len());
                            // Append the output chunk to the accumulated vectors
                            for ch in 0..num_channels {
                                if ch < output_chunk.len() { // Check channel exists in output
                                    accumulated_output_vecs[ch].extend_from_slice(&output_chunk[ch]);
                                }
                            }
                        } else {
                             trace!(target: LOG_TARGET, "Resampler output chunk is empty.");
                        }
                    }
                    Err(e) => {
                        error!(target: LOG_TARGET, "Resampling failed during chunk processing: {}", e);
                        // Decide how to handle: skip entire buffer or just this chunk? Let's skip the buffer.
                        return Ok(None); // Indicate skip for the whole buffer on chunk error
                    }
                }
                processed_frames = end_frame;
            }

            // 3. Convert the accumulated resampled f32 vectors to s16
            trace!(target: LOG_TARGET, "Converting accumulated resampled output ({} frames) to S16...", accumulated_output_vecs.get(0).map_or(0, |v| v.len()));
            s16_vec = match sample_converter::convert_f32_vecs_to_s16(accumulated_output_vecs) {
                Ok(vec) => vec,
                Err(e) => {
                    warn!(target: LOG_TARGET, "Failed to convert accumulated resampled F32 buffer to S16: {}. Skipping buffer.", e);
                    return Ok(None); // Indicate skip
                }
            };

        } else { // No resampler needed
            trace!(target: LOG_TARGET, "No resampling needed, converting directly to S16...");
            s16_vec = match sample_converter::convert_buffer_to_s16(audio_buffer) {
                Ok(vec) => vec,
                Err(e) => {
                    warn!(target: LOG_TARGET, "Failed to convert buffer to S16: {}. Skipping buffer.", e);
                    return Ok(None); // Indicate skip
                }
            };
        }

        // --- Check if buffer is empty ---
        if s16_vec.is_empty() {
            trace!(target: LOG_TARGET, "Skipping empty buffer after conversion/resampling.");
            return Ok(None); // Indicate skip
        }

        Ok(Some(s16_vec)) // Return the processed S16 buffer
    }
} // End impl PlaybackOrchestrator


#[async_trait]
impl AudioPlaybackControl for PlaybackOrchestrator {
    #[instrument(skip(self, on_finish), fields(url))]
    async fn play(
        &mut self,
        url: &str,
        total_duration_ticks: Option<i64>,
        on_finish: OnFinishCallback,
        shutdown_rx: broadcast::Receiver<()>, // Add shutdown_rx parameter (removed mut)
    ) -> Result<(), AudioError> {
        info!(target: LOG_TARGET, "AudioPlaybackControl::play called.");
        // Remove the dummy channel creation, use the passed-in receiver
        // let (_dummy_shutdown_tx, shutdown_rx) = broadcast::channel(1); // REMOVED

        // Pass the received shutdown_rx to the internal method
        self.internal_stream_decode_and_play(url, total_duration_ticks, shutdown_rx, on_finish).await
    }

    #[instrument(skip(self))]
    async fn pause(&mut self) -> Result<(), AudioError> {
        info!(target: LOG_TARGET, "AudioPlaybackControl::pause called.");
        // This relies on the shared pause_state being set externally.
        // The playback loop handles calling alsa_handler.pause() based on the state.
        // We might want to directly call alsa_handler.pause here if immediate effect is needed,
        // but coordinating with the loop's state check is complex.
        // For now, assume external state management via set_pause_state_tracker is sufficient.
        if self.pause_state.is_none() {
             warn!(target: LOG_TARGET, "Pause called, but pause state tracker is not set.");
             return Err(AudioError::InvalidState("Pause state tracker not configured".to_string()));
        }
        // Set the shared state to true. The loop will detect this and pause ALSA.
        let mut guard = self.pause_state.as_ref().unwrap().lock().await;
        *guard = true;
        Ok(())
    }

    #[instrument(skip(self))]
    async fn resume(&mut self) -> Result<(), AudioError> {
        info!(target: LOG_TARGET, "AudioPlaybackControl::resume called.");
        // Similar logic to pause: set the shared state, the loop handles ALSA resume.
        if self.pause_state.is_none() {
             warn!(target: LOG_TARGET, "Resume called, but pause state tracker is not set.");
             return Err(AudioError::InvalidState("Pause state tracker not configured".to_string()));
        }
        // Set the shared state to false. The loop will detect this and resume ALSA.
        let mut guard = self.pause_state.as_ref().unwrap().lock().await;
        *guard = false;
        Ok(())
    }

    // Removed orphaned `stop` method implementation


    #[instrument(skip(self))]
    async fn get_current_position_ticks(&self) -> Result<i64, AudioError> {
        trace!(target: LOG_TARGET, "AudioPlaybackControl::get_current_position_ticks called.");
        if let Some(progress_arc) = &self.progress_info {
            let progress_guard = progress_arc.lock().await;
            let position_ticks = (progress_guard.current_seconds * 10_000_000.0) as i64;
            trace!(target: LOG_TARGET, "Returning position_ticks: {}", position_ticks);
            Ok(position_ticks)
        } else {
            trace!(target: LOG_TARGET, "Progress info not available, returning 0 ticks.");
            // Return 0 or an error if progress isn't available? Let's return 0.
            Ok(0)
            // Err(AudioError::InvalidState("Progress tracker not configured".to_string()))
        }
    }

    // --- Volume/Mute/Seek method implementations removed ---

    // /// Sets the callback to be invoked when the current track finishes playing naturally.
    // #[instrument(skip(self, callback))]
    // async fn set_on_finish_callback(&mut self, callback: OnFinishCallback) {
    //     info!(target: LOG_TARGET, "AudioPlaybackControl::set_on_finish_callback called.");
    //     self.on_finish_callback = Some(callback);
    // }

    /// Sets the shared pause state tracker.
    fn set_pause_state_tracker(&mut self, state: Arc<TokioMutex<bool>>) {
        debug!(target: LOG_TARGET, "Pause state tracker configured via trait method.");
        self.pause_state = Some(state);
    }

    /// Sets the shared progress tracker.
    fn set_progress_tracker(&mut self, tracker: SharedProgress) {
        debug!(target: LOG_TARGET, "Progress tracker configured via trait method.");
        self.progress_info = Some(tracker);
    }

    /// Performs a full shutdown of the audio backend.
    #[instrument(skip(self))]
    async fn shutdown(&mut self) -> Result<(), AudioError> {
        info!(target: LOG_TARGET, "AudioPlaybackControl::shutdown called (delegating to PlaybackOrchestrator::shutdown).");
        // Call the existing async shutdown method which now uses spawn_blocking internally
        PlaybackOrchestrator::shutdown(self).await
    }
}


impl Drop for PlaybackOrchestrator {
    fn drop(&mut self) {
        // IMPORTANT: Avoid calling potentially blocking or async operations here.
        // Rely on the explicit async `shutdown()` method for proper cleanup.
        // If `shutdown()` was not called, resources like the ALSA handler
        // might not be cleaned up gracefully (especially the blocking close).
        // Calling blocking code here can lead to panics or deadlocks.
        debug!(target: LOG_TARGET, "Dropping PlaybackOrchestrator. Explicit async shutdown() is required for graceful ALSA cleanup.");
        // DO NOT call self.shutdown() or any potentially blocking methods here.
    }
}

// Helper extension trait for DecodedBufferAndTimestamp
trait DecodedBufferTimestampExt {
    fn type_id(&self) -> TypeId;
    fn timestamp(&self) -> u64;
}

impl DecodedBufferTimestampExt for DecodedBufferAndTimestamp {
    fn type_id(&self) -> TypeId {
        match self {
            DecodedBufferAndTimestamp::U8(_, _) => TypeId::of::<u8>(),
            DecodedBufferAndTimestamp::S16(_, _) => TypeId::of::<i16>(),
            DecodedBufferAndTimestamp::S24(_, _) => TypeId::of::<i32>(), // S24 uses i32
            DecodedBufferAndTimestamp::S32(_, _) => TypeId::of::<i32>(),
            DecodedBufferAndTimestamp::F32(_, _) => TypeId::of::<f32>(),
            DecodedBufferAndTimestamp::F64(_, _) => TypeId::of::<f64>(),
        }
    }

    fn timestamp(&self) -> u64 {
        match self {
            DecodedBufferAndTimestamp::U8(_, ts) => *ts,
            DecodedBufferAndTimestamp::S16(_, ts) => *ts,
            DecodedBufferAndTimestamp::S24(_, ts) => *ts,
            DecodedBufferAndTimestamp::S32(_, ts) => *ts,
            DecodedBufferAndTimestamp::F32(_, ts) => *ts,
            DecodedBufferAndTimestamp::F64(_, ts) => *ts,
        }
    }
}
