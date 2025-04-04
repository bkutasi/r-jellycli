// src/player/audio_task_manager.rs

use crate::audio::playback::AudioPlaybackControl;
use crate::player::{PlayerCommand, PLAYER_LOG_TARGET}; // Use PlayerCommand from parent re-export
// use std::sync::Arc; // Unused
use std::time::Duration as StdDuration;
use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinHandle;
use tracing::{debug, error, info, instrument, trace, warn};

/// Manages a single audio playback task.
#[derive(Debug)] // Add Debug derive
pub struct AudioTaskManager {
    task_handle: JoinHandle<()>,
    shutdown_tx: broadcast::Sender<()>,
    item_id: String, // Store item ID for logging
}

impl AudioTaskManager {
    /// Sends the shutdown signal to the managed task.
    fn signal_shutdown(&mut self) {
        debug!(target: PLAYER_LOG_TARGET, item_id = %self.item_id, "Sending shutdown signal to audio task.");
        // Send signal, ignore error if receiver is already gone (task might have finished)
        if let Err(e) = self.shutdown_tx.send(()) {
            // This is often expected if the task finished naturally before stop was called
            trace!(target: PLAYER_LOG_TARGET, item_id = %self.item_id, "Failed to send shutdown signal (receiver likely dropped): {}", e);
        }
    }

    /// Waits for the managed task to complete with a timeout.
    /// Consumes the manager instance.
    #[instrument(skip(self), fields(item_id = %self.item_id))]
    pub async fn await_completion(self) {
        debug!(target: PLAYER_LOG_TARGET, "Waiting for audio task to finish...");
        let timeout_duration = StdDuration::from_secs(2); // Reduced timeout to 2s for faster exit on Ctrl+C
        match tokio::time::timeout(timeout_duration, self.task_handle).await {
            Ok(Ok(())) => {
                info!(target: PLAYER_LOG_TARGET, item_id = %self.item_id, "Audio task finished gracefully.");
            }
            Ok(Err(e)) => {
                error!(target: PLAYER_LOG_TARGET, item_id = %self.item_id, "Audio task panicked: {:?}", e);
                // Propagate panic? Or just log?
            }
            Err(_) => {
                error!(target: PLAYER_LOG_TARGET, item_id = %self.item_id, "Timeout waiting for audio task to finish after {:?}. Task might be stuck.", timeout_duration);
                // Consider aborting the handle if timeout occurs?
                // self.task_handle.abort(); // Aborting is unstable and might leave resources hanging
            }
        }
    }

    /// Stops the managed task by sending a shutdown signal and awaiting completion.
    /// Consumes the manager instance.
    #[instrument(skip(self), fields(item_id = %self.item_id))]
    pub async fn stop_task(mut self) {
        info!(target: PLAYER_LOG_TARGET, "Stopping audio task manager...");
        let item_id = self.item_id.clone(); // Clone item_id before consuming self
        self.signal_shutdown();
        self.await_completion().await;
        info!(target: PLAYER_LOG_TARGET, item_id = %item_id, "Audio task manager stop sequence complete."); // Use the cloned item_id
    }

    /// Returns a reference to the JoinHandle for polling in select!
    pub fn handle(&mut self) -> &mut JoinHandle<()> {
        &mut self.task_handle
    }

    /// Returns the item ID associated with this task manager.
    pub fn item_id(&self) -> &str {
        &self.item_id
    }
}

/// Spawns a new Tokio task to handle audio playback.
#[instrument(skip(backend, on_finish_callback, internal_cmd_tx), fields(item_id = %item_id_clone, stream_url = %stream_url_clone))]
pub fn spawn_playback_task(
    mut backend: Box<dyn AudioPlaybackControl>, // Takes ownership of the backend
    stream_url_clone: String,
    item_id_clone: String,
    item_runtime_ticks: Option<i64>,
    on_finish_callback: Box<dyn FnOnce() + Send + Sync + 'static>,
    internal_cmd_tx: mpsc::Sender<PlayerCommand>, // Needed for the finish callback
) -> AudioTaskManager {
    let (shutdown_tx, shutdown_rx) = broadcast::channel::<()>(1); // Keep the receiver
    let item_id_for_struct = item_id_clone.clone(); // Clone before moving into spawn

    info!(target: PLAYER_LOG_TARGET, "Spawning async task for audio playback of item {}", item_id_clone);
    let task_handle = tokio::spawn(async move {
        // This async block runs in a separate Tokio task.
        // It owns the `backend` instance.
        let task_item_id = item_id_clone.clone(); // Clone for logging within the task
        debug!(target: PLAYER_LOG_TARGET, item_id = %task_item_id, "[Audio Task] Started.");

        // Create the finish callback specific to this task
        let finish_callback_for_task = {
            let cmd_tx = internal_cmd_tx.clone();
            let item_id = task_item_id.clone();
             Box::new(move || {
                info!(target: PLAYER_LOG_TARGET, item_id = %item_id, "[Audio Task] Finished track naturally. Sending TrackFinished command.");
                if let Err(e) = cmd_tx.try_send(PlayerCommand::TrackFinished) {
                    error!(target: PLAYER_LOG_TARGET, item_id = %item_id, "[Audio Task] Failed to send TrackFinished command: {}", e);
                }
                // Execute the original callback passed from the player if needed
                 on_finish_callback();
            })
        };


        // Call the backend's async play method
        // The shutdown_rx is now handled internally by the backend implementation (e.g., playback_loop)
        // Pass the actual shutdown_rx to the backend's play method
        let play_result = backend
            .play(&stream_url_clone, item_runtime_ticks, finish_callback_for_task, shutdown_rx) // Pass shutdown_rx
            .await;

        // --- Task Cleanup (within the spawned task) ---
        match play_result {
            Ok(()) => info!(target: PLAYER_LOG_TARGET, item_id = %task_item_id, "[Audio Task] Playback finished successfully."),
            Err(e) => error!(target: PLAYER_LOG_TARGET, item_id = %task_item_id, "[Audio Task] Playback failed: {}", e),
            // TODO: Consider sending an error update back to the main Player task?
        }

        // Explicitly shutdown the backend instance within the task before exiting.
        debug!(target: PLAYER_LOG_TARGET, item_id = %task_item_id, "[Audio Task] Shutting down backend instance...");
        if let Err(e) = backend.shutdown().await {
            error!(target: PLAYER_LOG_TARGET, item_id = %task_item_id, "[Audio Task] Error shutting down audio backend instance: {}", e);
        } else {
            info!(target: PLAYER_LOG_TARGET, item_id = %task_item_id, "[Audio Task] Backend instance shutdown complete.");
        }
        debug!(target: PLAYER_LOG_TARGET, item_id = %task_item_id, "[Audio Task] Finished.");
        // Task implicitly returns ()
    });

    AudioTaskManager {
        task_handle,
        shutdown_tx,
        item_id: item_id_for_struct, // Use the cloned value
    }
}