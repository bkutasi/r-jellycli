
use crate::audio::playback::AudioPlaybackControl;
use crate::player::{PlayerCommand, PLAYER_LOG_TARGET};
use std::time::Duration as StdDuration;
use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinHandle;
use tracing::{debug, error, info, instrument, trace, warn};

/// Manages a single audio playback task.
#[derive(Debug)]
pub struct AudioTaskManager {
    task_handle: JoinHandle<Result<(), crate::audio::error::AudioError>>,
    shutdown_tx: broadcast::Sender<()>,
    item_id: String,
}

impl AudioTaskManager {
    /// Sends the shutdown signal to the managed task.
    fn signal_shutdown(&mut self) {
        debug!(target: PLAYER_LOG_TARGET, item_id = %self.item_id, "Sending shutdown signal to audio task.");
        if let Err(e) = self.shutdown_tx.send(()) {
            trace!(target: PLAYER_LOG_TARGET, item_id = %self.item_id, "Failed to send shutdown signal (receiver likely dropped): {}", e);
        }
    }

    /// Waits for the managed task to complete with a timeout.
    #[instrument(skip(self), fields(item_id = %self.item_id))]
    pub async fn await_completion(mut self) {
        debug!(target: PLAYER_LOG_TARGET, "Waiting for audio task to finish...");
        let timeout_duration = StdDuration::from_secs(5);

        tokio::select! {
            biased; // Prioritize checking the result if ready
            join_result = &mut self.task_handle => {
                match join_result {
                    Ok(Ok(())) => {
                        // Task joined successfully, and the inner play() returned Ok(())
                        info!(target: PLAYER_LOG_TARGET, item_id = %self.item_id, "Audio task finished successfully.");
                    }
                    Ok(Err(audio_err)) => {
                         // Task joined successfully, but the inner play() returned an error
                        error!(target: PLAYER_LOG_TARGET, item_id = %self.item_id, "Audio task finished with error: {}", audio_err);
                    }
                    Err(join_err) => {
                        // Task failed to join (panic, cancellation, etc.)
                        if join_err.is_panic() {
                             error!(target: PLAYER_LOG_TARGET, item_id = %self.item_id, "Audio task panicked: {:?}", join_err);
                        } else if join_err.is_cancelled() {
                             info!(target: PLAYER_LOG_TARGET, item_id = %self.item_id, "Audio task was cancelled (likely aborted after timeout or explicit stop).");
                        } else {
                             error!(target: PLAYER_LOG_TARGET, item_id = %self.item_id, "Audio task join error: {:?}", join_err);
                        }
                    }
                }
            }
            _ = tokio::time::sleep(timeout_duration) => {
                error!(target: PLAYER_LOG_TARGET, item_id = %self.item_id, "Timeout waiting for audio task to finish after {:?}. Aborting task.", timeout_duration);
                self.task_handle.abort();
            }
        }
    }

    /// Stops the managed task by sending a shutdown signal and awaiting completion.
    #[instrument(skip(self), fields(item_id = %self.item_id))]
    pub async fn stop_task(mut self) {
        info!(target: PLAYER_LOG_TARGET, "Stopping audio task manager...");
        let item_id = self.item_id.clone();
        self.signal_shutdown();
        self.await_completion().await;
        info!(target: PLAYER_LOG_TARGET, item_id = %item_id, "Audio task manager stop sequence complete.");
    }

    // This method might need adjustment depending on how the handle is used elsewhere.
    // For now, let's keep the signature but acknowledge it might need changes if callers
    // rely on the specific type `JoinHandle<()>`.
    // If direct access isn't needed, consider removing this method.
    pub fn handle(&mut self) -> &mut JoinHandle<Result<(), crate::audio::error::AudioError>> {
        &mut self.task_handle
    }

    /// Returns the item ID associated with this task manager.
    pub fn item_id(&self) -> &str {
        &self.item_id
    }
}

/// Spawns a new Tokio task to handle audio playback.
#[instrument(skip(shared_backend, on_finish_callback, internal_cmd_tx), fields(item_id = %item_id_clone, stream_url = %stream_url_clone))]
pub fn spawn_playback_task(
    shared_backend: std::sync::Arc<tokio::sync::Mutex<crate::audio::PlaybackOrchestrator>>,
    stream_url_clone: String,
    item_id_clone: String,
    item_runtime_ticks: Option<i64>,
    on_finish_callback: Box<dyn FnOnce() + Send + Sync + 'static>,
    internal_cmd_tx: mpsc::Sender<PlayerCommand>,
) -> AudioTaskManager {
    let (shutdown_tx, shutdown_rx) = broadcast::channel::<()>(1);
    let item_id_for_struct = item_id_clone.clone();

    info!(target: PLAYER_LOG_TARGET, "Spawning async task for audio playback of item {}", item_id_clone);
    let task_handle = tokio::spawn(async move {
        let task_item_id = item_id_clone.clone();
        debug!(target: PLAYER_LOG_TARGET, item_id = %task_item_id, "[Audio Task] Started.");

        let finish_callback_for_task = {
            let cmd_tx = internal_cmd_tx.clone();
            let item_id = task_item_id.clone();
             Box::new(move || {
                info!(target: PLAYER_LOG_TARGET, item_id = %item_id, "[Audio Task] Finished track naturally. Sending TrackFinished command.");
                if let Err(e) = cmd_tx.try_send(PlayerCommand::TrackFinished) {
                    error!(target: PLAYER_LOG_TARGET, item_id = %item_id, "[Audio Task] Failed to send TrackFinished command: {}", e);
                }
                 on_finish_callback();
            })
        };


        let play_result = {
            let mut backend_guard = shared_backend.lock().await;
            backend_guard
                .play(&stream_url_clone, item_runtime_ticks, finish_callback_for_task, shutdown_rx)
                .await
        };

        // Log the result here within the task
        match &play_result {
            Ok(()) => info!(target: PLAYER_LOG_TARGET, item_id = %task_item_id, "[Audio Task] Playback finished successfully."),
            Err(e) => error!(target: PLAYER_LOG_TARGET, item_id = %task_item_id, "[Audio Task] Playback failed: {}", e),
        }
        debug!(target: PLAYER_LOG_TARGET, item_id = %task_item_id, "[Audio Task] Finished.");

        // Return the result so the JoinHandle reflects success/failure
        play_result
    });

    AudioTaskManager {
        task_handle,
        shutdown_tx,
        item_id: item_id_for_struct,
    }
}