// Removed log::trace import

use std::sync::Arc; // Removed Mutex import from std::sync
use std::time::Duration as StdDuration;
use tokio::sync::{mpsc, Mutex as TokioMutex, broadcast}; // Use TokioMutex alias
use tokio::task::JoinHandle;
use crate::audio::{PlaybackOrchestrator, PlaybackProgressInfo}; // Renamed AlsaPlayer
use crate::audio::AudioError;

use crate::jellyfin::api::{JellyfinClient, JellyfinError}; // Import JellyfinError
use crate::jellyfin::models::MediaItem;
use crate::jellyfin::{PlaybackStartReport, PlaybackStopReport, PlaybackReportBase, QueueItem, PlaybackStoppedInfoInner, PlaybackProgressReport}; // Use re-exported path
use crate::jellyfin::websocket::PlayerStateUpdate; // Keep for potential UI updates
use tracing::{debug, error, info, warn, trace}; // Replaced log with tracing
use tracing::instrument;

// Player struct wraps the audio player and adds remote control capabilities
pub struct Player {
    alsa_player: Option<Arc<TokioMutex<PlaybackOrchestrator>>>, // Use TokioMutex
    current_item_id: Option<String>,
    position_ticks: i64,
    is_paused: Arc<TokioMutex<bool>>, // Changed to shared state
    is_playing: bool,
    volume: i32,
    is_muted: bool,
    queue: Vec<MediaItem>,
    current_queue_index: usize,
    // Add sender for state updates
    update_tx: Option<mpsc::UnboundedSender<PlayerStateUpdate>>,
    // Jellyfin client for fetching stream URLs
    jellyfin_client: Option<JellyfinClient>, // Needs to be set
    // Shared progress state
    current_progress: Option<Arc<TokioMutex<PlaybackProgressInfo>>>, // Use TokioMutex
    // Handles for background tasks
    playback_task_handle: Option<JoinHandle<Result<(), crate::audio::AudioError>>>,
    reporter_task_handle: Option<JoinHandle<()>>,
    // Shutdown signal for playback task
    playback_shutdown_tx: Option<broadcast::Sender<()>>,
    // Shutdown signal for reporter task
    reporter_shutdown_tx: Option<broadcast::Sender<()>>, // Added
    // Store the configured ALSA device name
    alsa_device_name: String,
}

impl Player {
    #[instrument(skip_all)]
    pub fn new() -> Self {
        Player {
            alsa_player: None,
            current_item_id: None,
            position_ticks: 0,
            is_paused: Arc::new(TokioMutex::new(false)), // Initialize shared state
            is_playing: false,
            volume: 100,
            is_muted: false,
            queue: Vec::new(),
            current_queue_index: 0,
            update_tx: None,
            jellyfin_client: None,
            current_progress: None,
            playback_task_handle: None,
            reporter_task_handle: None,
            playback_shutdown_tx: None,
            reporter_shutdown_tx: None, // Added init
            alsa_device_name: String::new(), // Initialize device name
        }
    }

    // Method to set the Jellyfin client
    pub fn set_jellyfin_client(&mut self, client: JellyfinClient) {
        info!("Jellyfin client configured.");
        self.jellyfin_client = Some(client);
    }

    // Keep set_alsa_player if needed for pre-creation, or remove if created on play
    pub fn set_alsa_player(&mut self, alsa_player: Arc<TokioMutex<PlaybackOrchestrator>>) { // Use TokioMutex
        info!("ALSA player instance set.");
        self.alsa_player = Some(alsa_player);
    }

    // Method to set the update sender
    #[instrument(skip_all)]
    pub fn set_update_sender(&mut self, tx: mpsc::UnboundedSender<PlayerStateUpdate>) {
        info!("State update sender configured.");
        self.update_tx = Some(tx);
    }

    // Send state update helper
    fn send_update(&self, update: PlayerStateUpdate) {
        if let Some(tx) = &self.update_tx {
            if let Err(e) = tx.send(update) {
                error!("[PLAYER] Failed to send state update: {}", e);
            }
        } else {
             warn!("[PLAYER] Cannot send state update: sender not configured.");
        }
    }

    // Method to store the ALSA device name
    pub fn set_alsa_device_name(&mut self, name: String) {
        info!("Setting ALSA device name to: {}", name);
        self.alsa_device_name = name;
    }


    pub fn set_current_item(&mut self, item: &MediaItem) { // Take full item
        self.current_item_id = Some(item.id.clone());
        self.position_ticks = 0;
        // Reset pause state when setting a new item
        let mut pause_guard = self.is_paused.blocking_lock(); // Use blocking lock as this isn't async
        *pause_guard = false;
        drop(pause_guard);
        self.is_playing = true;

        // Send Started update
        self.send_update(PlayerStateUpdate::Started { item: item.clone() });
    }

    pub fn update_position(&mut self, position_ticks: i64) {
        self.position_ticks = position_ticks;
        // Progress updates will be handled by a separate task later
    }

    // --- Add Getter methods needed by WebSocketHandler ---
    pub fn get_position(&self) -> i64 {
        self.position_ticks
    }

    pub fn get_volume(&self) -> i32 {
        self.volume
    }

    pub fn is_muted(&self) -> bool {
        self.is_muted
    }

    // Make this async as it now needs to lock the mutex
    pub async fn is_paused(&self) -> bool {
        *self.is_paused.lock().await
    }
    // --- End Getter methods ---

    #[instrument(skip(self))]
    pub async fn clear_queue(&mut self) {
        info!("Clearing playback queue");
        self.queue.clear();
        self.current_queue_index = 0;
        self.current_item_id = None;
        self.is_playing = false;
        // Reset pause state using async lock
        let mut pause_guard = self.is_paused.lock().await; // Use async lock
        *pause_guard = false;
        drop(pause_guard);
        
        // Stop current playback if any
        if let Some(alsa_player) = &self.alsa_player {
            let _player_guard = alsa_player.lock().await;
            // Call stop on the AlsaPlayer if it has such method
            // _player_guard.stop();
        }
    }

    pub fn add_items(&mut self, items: Vec<MediaItem>) {
        if items.is_empty() {
            info!("No items to add to queue");
            return;
        }

        info!("Adding {} items to the queue", items.len());
        for item in &items {
            debug!("[PLAYER] - Added: {} ({})", item.name, item.id); // Use debug for item details
        }
        
        self.queue.extend(items);
    }

    #[instrument(skip(self))]
    pub async fn play_from_start(&mut self) {
        if self.queue.is_empty() {
            warn!("[PLAYER] Cannot play, queue is empty");
            return;
        }

        self.current_queue_index = 0;
        if let Err(e) = self.play_current_queue_item().await {
            error!("[PLAYER] Failed to play item from start: {}", e);
            // Optionally reset state here if needed
            self.is_playing = false;
        }
    }

    // Make play_current_queue_item return Result
    #[instrument(skip(self), fields(queue_index = self.current_queue_index))]
    async fn play_current_queue_item(&mut self) -> Result<(), AudioError> { // Return Result
        // --- Stop existing playback first --- (REMOVED - Player::stop() handles this before PlayNow calls play_from_start -> play_current_queue_item)
        // self.stop_playback_tasks().await?; // Stop previous tasks if any, propagate error

        if self.current_queue_index >= self.queue.len() {
            warn!("[PLAYER] No item at index {} to play.", self.current_queue_index);
            self.is_playing = false; // Ensure state reflects no playback
            return Ok(()); // Not an error, just nothing to play
        }

        let item_to_play = self.queue[self.current_queue_index].clone();
        info!("Preparing to play: {} ({})", item_to_play.name, item_to_play.id);

        // --- Update Player State Immediately ---
        // Set state *before* getting URL or reporting start
        self.current_item_id = Some(item_to_play.id.clone());
        self.position_ticks = 0;
        // Reset pause state when starting new item
        *self.is_paused.lock().await = false;
        self.is_playing = true;

        // --- Get Stream URL ---
        let stream_url: String = match &self.jellyfin_client { // Added type annotation
            Some(client) => {
                // Assuming get_audio_stream_url exists and returns Result<String, Error>
                match client.get_stream_url(&item_to_play.id) { // Removed .await
                    Ok(url) => url,
                    Err(e) => { // 'e' is JellyfinError::Authentication or similar
                        error!("[PLAYER] Failed to get stream URL for {}: {}", item_to_play.id, e);
                        // TODO: Maybe try next item or send error update?
                        self.is_playing = false; // Reset playing state on error
                        // Map JellyfinError (likely Authentication) to AudioError::InvalidState
                        return Err(AudioError::InvalidState(format!(
                            "Failed to get stream URL (likely auth issue): {}",
                            e
                        )));
                    }
                }
            }
            None => {
                error!("[PLAYER] Jellyfin client not set, cannot get stream URL.");
                self.is_playing = false; // Reset playing state
                return Err(AudioError::InvalidState("Jellyfin client not configured".to_string()));
            }
        };
        debug!("[PLAYER] Got stream URL: {}", stream_url);


        // --- Report Playback Start via HTTP POST ---
        if let Some(client) = &self.jellyfin_client {
            let session_id = client.play_session_id().to_string(); // Get session ID from client
            let queue_items: Vec<QueueItem> = self.queue.iter().enumerate().map(|(idx, item)| QueueItem {
                id: item.id.clone(),
                playlist_item_id: format!("playlistItem{}", idx),
            }).collect();

            let report_base = PlaybackReportBase {
                queueable_media_types: vec!["Audio".to_string()],
                can_seek: true, // Assuming seek is generally possible
                item_id: item_to_play.id.clone(),
                media_source_id: item_to_play.id.clone(), // Use item ID as source ID like Go client
                position_ticks: 0, // Starts at 0
                volume_level: self.volume,
                is_paused: *self.is_paused.lock().await, // Read shared state
                is_muted: self.is_muted,
                play_method: "DirectPlay".to_string(),
                play_session_id: session_id,
                live_stream_id: None, // Go uses "", map to None
                playlist_length: item_to_play.run_time_ticks.unwrap_or(0), // Use item duration
                playlist_index: Some(self.current_queue_index as i32), // Current index
                shuffle_mode: "Sorted".to_string(), // TODO: Add shuffle state later
                now_playing_queue: queue_items,
            };

            let start_report = PlaybackStartReport { base: report_base };

            match client.report_playback_start(&start_report).await {
                Ok(_) => info!("Reported playback start successfully for item {}", item_to_play.id),
                Err(e) => error!("[PLAYER] Failed to report playback start for item {}: {}", item_to_play.id, e), // Log error, but don't stop playback
            }
        } else {
            error!("[PLAYER] Cannot report playback start: Jellyfin client not available.");
        }

        // Send internal update for UI etc. *after* reporting start
        self.send_update(PlayerStateUpdate::Started { item: item_to_play.clone() });
        // --- Setup for new playback ---
        let progress_info = Arc::new(TokioMutex::new(PlaybackProgressInfo::default())); // Use TokioMutex
        self.current_progress = Some(progress_info.clone());

        // --- Ensure we have a PlaybackOrchestrator instance ---
        // Since stop_playback_tasks might have taken the old one, create/set a new one.
        // This assumes we get the device name from somewhere (config/args).
        // Use the stored device name, falling back to "default" if not set (with error log)
        let device_name_to_use = if self.alsa_device_name.is_empty() {
            error!("[PLAYER] ALSA device name not set in Player state! Falling back to 'default'. This should be set during initialization.");
            "default" // Fallback
        } else {
            &self.alsa_device_name
        };
        info!("[PLAYER] Creating PlaybackOrchestrator for device: '{}'", device_name_to_use);
        let new_alsa_player = Arc::new(TokioMutex::new(PlaybackOrchestrator::new(device_name_to_use)));
        self.alsa_player = Some(new_alsa_player.clone()); // Set the new player instance
        info!("[PLAYER] Created and set new PlaybackOrchestrator for device '{}'", device_name_to_use);
        let alsa_player_arc = new_alsa_player; // Use the new instance

        // Set the progress tracker on the AlsaPlayer instance
        { // Scope for MutexGuard
            let mut alsa_player_guard = alsa_player_arc.lock().await;
            alsa_player_guard.set_progress_tracker(progress_info.clone());
            // Pass the shared pause state to the orchestrator
            alsa_player_guard.set_pause_state(self.is_paused.clone());
        }

        // Create shutdown channel for this playback instance
        let (playback_shutdown_tx, playback_shutdown_rx) = broadcast::channel(1);
        self.playback_shutdown_tx = Some(playback_shutdown_tx);

        // Create shutdown channel for the reporter task
        let (reporter_shutdown_tx, reporter_shutdown_rx) = broadcast::channel(1);
        self.reporter_shutdown_tx = Some(reporter_shutdown_tx); // Store sender


        // --- Spawn Playback Task ---
        let playback_handle = tokio::spawn(async move {
            let mut player_guard = alsa_player_arc.lock().await;
            // Pass the receiver for this specific playback
            player_guard.stream_decode_and_play(&stream_url, item_to_play.run_time_ticks, playback_shutdown_rx).await
        });
        self.playback_task_handle = Some(playback_handle);
        info!("Spawned playback task.");


        // --- Spawn Progress Reporter Task ---
        let reporter_handle = if let Some(client) = self.jellyfin_client.clone() { // Need client clone
            let weak_progress = Arc::downgrade(&progress_info); // Use weak ref to avoid cycles

            // Clone necessary state for the reporter task
            let initial_item_id = item_to_play.id.clone();
            let item_duration_ticks = item_to_play.run_time_ticks.unwrap_or(0);
            let session_id = client.play_session_id().to_string();
            let queue_items: Vec<QueueItem> = self.queue.iter().enumerate().map(|(idx, item)| QueueItem {
                id: item.id.clone(),
                playlist_item_id: format!("playlistItem{}", idx),
            }).collect();
            let initial_queue_index = self.current_queue_index;
            // TODO: Add shuffle state tracking later
            let shuffle_mode = "Sorted".to_string();

            // Pass the receiver for this specific reporter
            let mut reporter_shutdown_rx_clone = reporter_shutdown_rx; // Clone receiver for the task

            // Clone the shared pause state Arc for the reporter task
            let pause_state_for_reporter = self.is_paused.clone();
            // Capture volume and mute state (these don't need Arc currently)
            let initial_volume = self.volume;
            let initial_is_muted = self.is_muted;


            let handle = tokio::spawn(async move {
                let mut interval = tokio::time::interval(StdDuration::from_secs(1)); // Report every second
                info!("[Reporter] HTTP Progress reporter task started for item {}", initial_item_id);
                loop {
                    tokio::select! {
                        biased; // Check shutdown first
                        _ = reporter_shutdown_rx_clone.recv() => {
                            info!("[Reporter] Shutdown signal received. Stopping reporter.");
                            break;
                        }
                        _ = interval.tick() => {
                            if let Some(progress_arc) = weak_progress.upgrade() {
                                // Lock the async mutex to get current progress
                                let current_secs = progress_arc.lock().await.current_seconds;
                                // Note: Tokio's mutex lock doesn't return Result, it panics on poison.
                                // Removed the previous error handling for PoisonError.
                                // If the mutex is poisoned, the task will panic and stop.
                                let position_ticks = (current_secs * 10_000_000.0) as i64;

                                // Construct the full progress report
                                let report_base = PlaybackReportBase {
                                    queueable_media_types: vec!["Audio".to_string()],
                                    can_seek: true,
                                    item_id: initial_item_id.clone(),
                                    media_source_id: initial_item_id.clone(),
                                    position_ticks,
                                    volume_level: initial_volume, // Use captured volume
                                    is_paused: *pause_state_for_reporter.lock().await, // Read current pause state
                                    is_muted: initial_is_muted,   // Use captured state
                                    play_method: "DirectPlay".to_string(),
                                    play_session_id: session_id.clone(),
                                    live_stream_id: None,
                                    playlist_length: item_duration_ticks,
                                    playlist_index: Some(initial_queue_index as i32), // Use captured state
                                    shuffle_mode: shuffle_mode.clone(),
                                    now_playing_queue: queue_items.clone(), // Clone queue state
                                };
                                let progress_report = PlaybackProgressReport { base: report_base };

                                // Send report via HTTP POST
                                match client.report_playback_progress(&progress_report).await {
                                    Ok(_) => trace!("[Reporter] Reported progress successfully for item {}", initial_item_id),
                                    Err(JellyfinError::Network(e)) if e.is_timeout() => {
                                        warn!("[Reporter] Timeout reporting progress for item {}: {}", initial_item_id, e);
                                        // Continue trying on next tick
                                    }
                                    Err(e) => {
                                        error!("[Reporter] Failed to report progress for item {}: {}. Stopping reporter.", initial_item_id, e);
                                        break; // Stop reporting on persistent errors
                                    }
                                }
                            } else {
                                info!("[Reporter] Progress info dropped. Stopping reporter.");
                                break; // Stop if progress info is gone
                            }
                        }
                    }
                }
                info!("[Reporter] HTTP Progress reporter task stopped for item {}", initial_item_id);
            });
            Some(handle)
        } else {
            warn!("[PLAYER] Cannot start progress reporter: Jellyfin client not configured.");
            None
        };
        self.reporter_task_handle = reporter_handle;


        // --- Finalize State (already set earlier) ---
        // self.set_current_item(&item_to_play); // Removed: State set before reporting start
        Ok(()) // Indicate success
    }

    #[instrument(skip(self))]
    pub async fn play_pause(&mut self) {
        if self.is_playing {
            // Read current pause state
            let currently_paused = *self.is_paused.lock().await;
            if currently_paused {
                info!("Resumed playback");
                self.resume().await; // resume will set shared state to false and send update
            } else {
                info!("Paused playback");
                self.pause().await;
            }
        } else if !self.queue.is_empty() {
            // This case should ideally be handled by play_from_start or similar
            // If play_pause starts playback, ensure set_current_item is called
            info!("Started playback via play_pause");
            self.play_from_start().await;
        }
    }

    #[instrument(skip(self))]
    pub async fn pause(&mut self) {
        if !self.is_playing { return; }
        // Check current state before trying to pause
        let mut pause_guard = self.is_paused.lock().await;
        if *pause_guard { return; } // Already paused

        *pause_guard = true; // Set shared state to paused
        // Drop the guard before sending updates/logging to avoid holding lock
        drop(pause_guard);
        info!("State set to paused");
        // Send progress update reflecting paused state
        if let Some(id) = self.current_item_id.clone() {
             self.send_update(PlayerStateUpdate::Progress {
                 item_id: id,
                 position_ticks: self.position_ticks,
                 is_paused: true, // We just paused
                 volume: self.volume,       // Add player state
                 is_muted: self.is_muted,   // Add player state
             });
        }
        // No ALSA call needed here, the playback loop handles the shared state
    }

    #[instrument(skip(self))]
    pub async fn resume(&mut self) {
        if !self.is_playing { return; }
        // Check current state before trying to resume
        let mut pause_guard = self.is_paused.lock().await;
        if !*pause_guard { return; } // Already playing

        *pause_guard = false; // Set shared state to playing
        // Drop the guard before sending updates/logging
        drop(pause_guard);
        info!("State set to resumed");
        // Send progress update reflecting resumed state
        if let Some(id) = self.current_item_id.clone() {
             self.send_update(PlayerStateUpdate::Progress {
                 item_id: id,
                 position_ticks: self.position_ticks,
                 is_paused: false, // We just resumed
                 volume: self.volume,       // Add player state
                 is_muted: self.is_muted,   // Add player state
             });
        }
        // No ALSA call needed here, the playback loop handles the shared state
    }

    // Helper to stop playback tasks and shut down the associated orchestrator
    #[instrument(skip(self))]
    async fn stop_playback_tasks(&mut self) -> Result<(), AudioError> { // Return Result
        if self.playback_task_handle.is_none() && self.reporter_task_handle.is_none() {
            // No tasks running
            return Ok(()); // Nothing to do, return Ok
        }
        info!("Stopping playback and reporter tasks...");

        // Signal reporter task to stop first
        if let Some(tx) = self.reporter_shutdown_tx.take() {
            let _ = tx.send(()); // Send shutdown signal
            debug!("[PLAYER] Sent shutdown signal to reporter task.");
        }
        // Signal playback task to stop
        if let Some(tx) = self.playback_shutdown_tx.take() {
            let _ = tx.send(()); // Send shutdown signal
            debug!("[PLAYER] Sent shutdown signal to playback task.");
        }

        // Abort and wait for tasks to finish
        // Give them a moment to shut down gracefully before aborting
        tokio::time::sleep(StdDuration::from_millis(50)).await;

        if let Some(handle) = self.reporter_task_handle.take() {
             if !handle.is_finished() {
                 debug!("[PLAYER] Aborting reporter task...");
                 handle.abort();
             }
             match handle.await {
                 Ok(_) => debug!("[PLAYER] Reporter task finished."),
                 Err(e) if e.is_cancelled() => info!("Reporter task cancelled."),
                 Err(e) => error!("[PLAYER] Reporter task panicked or failed: {}", e),
             }
        }
        if let Some(handle) = self.playback_task_handle.take() {
            if !handle.is_finished() {
                debug!("[PLAYER] Aborting playback task...");
                handle.abort();
            }
            match handle.await {
                Ok(Ok(_)) => debug!("[PLAYER] Playback task finished gracefully."),
                Ok(Err(e)) => error!("[PLAYER] Playback task finished with error: {}", e),
                Err(e) if e.is_cancelled() => info!("Playback task cancelled."),
                Err(e) => error!("[PLAYER] Playback task panicked or failed: {}", e),
            }
        }

        // --- Explicitly Shutdown the Orchestrator ---
        // Take the orchestrator instance associated with the stopped tasks.
        // Assuming self.alsa_player holds the *current* orchestrator.
        if let Some(alsa_player_arc) = self.alsa_player.take() { // Take ownership
            info!("Shutting down PlaybackOrchestrator for stopped tasks...");
            // Lock the orchestrator to call shutdown.
            // Note: PlaybackOrchestrator::shutdown is async.
            // We need to ensure the orchestrator isn't locked elsewhere.
            // Let's assume we can get the lock here.
            let mut player_guard = alsa_player_arc.lock().await;
            if let Err(e) = player_guard.shutdown().await {
                error!("[PLAYER] Error during PlaybackOrchestrator shutdown while stopping tasks: {}", e);
                // Decide if this should be a fatal error for stop_playback_tasks
                // return Err(e); // Option: Propagate the error
            } else {
                info!("PlaybackOrchestrator shutdown complete during task stop.");
            }
            // Drop the guard explicitly
            drop(player_guard);
            // Let play_current_queue_item handle creating/setting a new one if needed.
        } else {
            info!("No PlaybackOrchestrator instance found to shut down during task stop.");
        }
        // --- End Orchestrator Shutdown ---

        self.current_progress = None; // Clear progress state
        info!("Playback and reporter tasks stopped.");
        Ok(()) // Indicate success
    }

    /// Helper function to report playback stopped state to Jellyfin.
    #[instrument(skip(self, stopped_item_id, final_ticks))]
    async fn _report_playback_stopped(&self, stopped_item_id: Option<String>, final_ticks: i64) {
        if let (Some(client), Some(id)) = (&self.jellyfin_client, stopped_item_id) {
            let session_id = client.play_session_id().to_string();
            // Generate queue items based on the current queue state
            let queue_items: Vec<QueueItem> = self.queue.iter().enumerate().map(|(idx, item)| QueueItem {
                id: item.id.clone(),
                playlist_item_id: format!("playlistItem{}", idx),
            }).collect();

            // Capture necessary state for the report
            let report_base = PlaybackReportBase {
                queueable_media_types: vec!["Audio".to_string()],
                can_seek: true,
                item_id: id.clone(),
                media_source_id: id.clone(),
                position_ticks: final_ticks, // Use provided final position
                volume_level: self.volume,
                is_paused: false, // Stopped means not paused (read from shared state is unnecessary here)
                is_muted: self.is_muted,
                play_method: "DirectPlay".to_string(),
                play_session_id: session_id,
                live_stream_id: None,
                // Find the original item to get its duration for playlist_length
                playlist_length: self.queue.iter().find(|item| item.id == id).and_then(|item| item.run_time_ticks).unwrap_or(0),
                playlist_index: Some(self.current_queue_index as i32), // Use current index
                shuffle_mode: "Sorted".to_string(), // TODO: Add shuffle state
                now_playing_queue: queue_items,
            };

            let stop_report = PlaybackStopReport {
                base: report_base,
                playback_stopped_info: PlaybackStoppedInfoInner {
                    played_to_completion: false, // Explicit stop/shutdown is not completion
                },
            };

            match client.report_playback_stopped(&stop_report).await {
                Ok(_) => info!("Reported playback stopped successfully for item {}", id),
                Err(e) => error!("[PLAYER] Failed to report playback stopped for item {}: {}", id, e),
            }
        } else {
            error!("[PLAYER] Cannot report playback stopped: Jellyfin client or Item ID not available.");
        }
    }


    #[instrument(skip(self))]
    pub async fn stop(&mut self) {
        if !self.is_playing {
            debug!("[PLAYER] Stop called but not playing.");
            return;
        }
        info!("Stop requested.");

        let stopped_item_id = self.current_item_id.clone();
        // Get final position from shared state if possible, otherwise use last known
        let final_ticks = if let Some(progress_arc) = self.current_progress.as_ref() {
            let p_info = progress_arc.lock().await; // Lock the async mutex
            (p_info.current_seconds * 10_000_000.0) as i64 // Calculate ticks from locked info
        } else {
            self.position_ticks // Fallback if progress info is None
        };

        // Report Playback Stopped *before* stopping tasks
        self._report_playback_stopped(stopped_item_id.clone(), final_ticks).await;

        // Stop background tasks
        if let Err(e) = self.stop_playback_tasks().await {
            error!("[PLAYER] Error stopping playback tasks during explicit stop: {}", e);
            // Handle error as needed, maybe try to continue state update
        }

        // Update state *after* stopping tasks and reporting
        self.is_playing = false;
        *self.is_paused.lock().await = false; // Reset shared state
        self.current_item_id = None;
        self.position_ticks = 0; // Reset position after stopping

    }
    #[instrument(skip(self))]
    pub async fn next(&mut self) {
        if self.queue.is_empty() {
            warn!("[PLAYER] Queue is empty, cannot skip to next item");
            return;
        }

        if self.current_queue_index >= self.queue.len() - 1 {
            info!("Already at the end of the queue");
            return;
        }

        self.current_queue_index += 1;
        info!("Skipping to next item (index: {})", self.current_queue_index);
        if let Err(e) = self.play_current_queue_item().await {
             error!("[PLAYER] Failed to play next item: {}", e);
             // TODO: Handle error, maybe stop playback or try next?
        }
    }

    #[instrument(skip(self))]
    pub async fn previous(&mut self) {
        if self.queue.is_empty() {
            warn!("[PLAYER] Queue is empty, cannot skip to previous item");
            return;
        }

        if self.current_queue_index == 0 {
            info!("Already at the beginning of the queue");
            return;
        }

        self.current_queue_index -= 1;
        info!("Skipping to previous item (index: {})", self.current_queue_index);
        if let Err(e) = self.play_current_queue_item().await {
             error!("[PLAYER] Failed to play previous item: {}", e);
             // TODO: Handle error
        }
    }

    #[instrument(skip(self), fields(volume))]
    pub async fn set_volume(&mut self, volume: u8) {
        let clamped_volume = volume.min(100); // Ensure volume is 0-100
        info!("Setting volume to {}", clamped_volume);
        self.volume = clamped_volume as i32;
        // Volume changes are implicitly reported via Progress updates
        // which fetch the latest state in the WS handler.
        // No specific VolumeChanged update needed here.
        // Consider sending a Progress update if immediate feedback is desired.
        // self.send_progress_update().await; // Example if needed
        // TODO: Implement actual ALSA volume control
    }

    #[instrument(skip(self))]
    pub async fn toggle_mute(&mut self) {
        self.is_muted = !self.is_muted;
        info!("Mute toggled: {}", self.is_muted);
        // Mute changes are implicitly reported via Progress updates
        // which fetch the latest state in the WS handler.
        // No specific VolumeChanged update needed here.
        // Consider sending a Progress update if immediate feedback is desired.
        // self.send_progress_update().await; // Example if needed
        // TODO: Implement actual ALSA mute control
    }

    #[instrument(skip(self), fields(position_ticks))]
    pub async fn seek(&mut self, position_ticks: i64) {
        info!("Seeking to position: {}", position_ticks);
        self.position_ticks = position_ticks;
        // Send progress update after seek
        if let Some(id) = self.current_item_id.clone() {
             self.send_update(PlayerStateUpdate::Progress {
                 item_id: id,
                 position_ticks: self.position_ticks,
                 is_paused: *self.is_paused.lock().await, // Read shared state
                 volume: self.volume,       // Add player state
                 is_muted: self.is_muted,   // Add player state
             });
        }
        // TODO: Implement actual ALSA seek functionality
    }


    /// Performs graceful shutdown of the Player and its components.
    /// This includes stopping background tasks and shutting down the audio player.
    #[instrument(skip(self))]
    pub async fn shutdown(&mut self) {
        info!("Initiating shutdown...");

        // Capture state *before* stopping tasks
        let item_id_before_shutdown = self.current_item_id.clone();
        let final_ticks_before_shutdown = if let Some(progress_arc) = self.current_progress.as_ref() {
             // Try to get the most recent position from the shared state
             let p_info = progress_arc.lock().await;
             (p_info.current_seconds * 10_000_000.0) as i64
         } else {
             // Fallback to the last known position in the Player struct
             self.position_ticks
         };

        // Report playback stopped if an item was playing
        if item_id_before_shutdown.is_some() {
            info!("Reporting playback stopped during shutdown...");
            self._report_playback_stopped(item_id_before_shutdown, final_ticks_before_shutdown).await;
        } else {
            info!("No active item playing, skipping stop report during shutdown.");
        }

        // 1. Stop background tasks (playback and reporter)
        if let Err(e) = self.stop_playback_tasks().await {
            error!("[PLAYER] Error stopping playback tasks during player shutdown: {}", e);
            // Continue shutdown process despite error?
        }

        // 2. Shutdown the PlaybackOrchestrator
        // Note: stop_playback_tasks now handles shutting down the orchestrator it stopped.
        // If a player wasn't running, self.alsa_player might still exist, but shouldn't need explicit shutdown here.
        // Let's ensure it's cleared if it exists.
        if self.alsa_player.is_some() {
             debug!("[PLAYER] Clearing remaining PlaybackOrchestrator instance during shutdown (already stopped if was running).");
             self.alsa_player = None;
        }
        // if let Some(alsa_player_arc) = self.alsa_player.take() { // Take ownership
        //     info!("Shutting down PlaybackOrchestrator...");
        //     let mut player_guard = alsa_player_arc.lock().await;
        //     if let Err(e) = player_guard.shutdown().await {
        //         error!("[PLAYER] Error during PlaybackOrchestrator shutdown: {}", e);
        //     } else {
        //         info!("PlaybackOrchestrator shutdown complete.");
        //     }
        //     // Drop the guard explicitly
        //     drop(player_guard);
        // } else {
        //     info!("No PlaybackOrchestrator instance to shut down.");
        // }

        // 3. Clear remaining state (optional, as tasks are stopped)
        self.is_playing = false;
        // Reset shared state during shutdown
        *self.is_paused.lock().await = false;
        self.current_item_id = None;
        self.queue.clear();
        self.current_queue_index = 0;

        info!("Shutdown complete.");
    }

    }


