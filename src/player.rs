// Removed log::trace import

use std::sync::Arc; // Removed Mutex import from std::sync
use std::time::Duration as StdDuration;
use tokio::sync::{mpsc, Mutex as TokioMutex, broadcast}; // Use TokioMutex alias
use tokio::task::JoinHandle;
use crate::audio::{PlaybackOrchestrator, PlaybackProgressInfo}; // Renamed AlsaPlayer
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
    is_paused: bool,
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
}

impl Player {
    #[instrument(skip_all)]
    pub fn new() -> Self {
        Player {
            alsa_player: None,
            current_item_id: None,
            position_ticks: 0,
            is_paused: false,
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


    pub fn set_current_item(&mut self, item: &MediaItem) { // Take full item
        self.current_item_id = Some(item.id.clone());
        self.position_ticks = 0;
        self.is_paused = false;
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

    pub fn is_paused(&self) -> bool {
        self.is_paused
    }
    // --- End Getter methods ---

    #[instrument(skip(self))]
    pub async fn clear_queue(&mut self) {
        info!("Clearing playback queue");
        self.queue.clear();
        self.current_queue_index = 0;
        self.current_item_id = None;
        self.is_playing = false;
        self.is_paused = false;
        
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
        self.play_current_queue_item().await;
    }

    #[instrument(skip(self), fields(queue_index = self.current_queue_index))]
    async fn play_current_queue_item(&mut self) {
        // --- Stop existing playback first ---
        self.stop_playback_tasks().await; // Stop previous tasks if any

        if self.current_queue_index >= self.queue.len() {
            warn!("[PLAYER] No item at index {} to play.", self.current_queue_index);
            self.is_playing = false; // Ensure state reflects no playback
            return;
        }

        let item_to_play = self.queue[self.current_queue_index].clone();
        info!("Preparing to play: {} ({})", item_to_play.name, item_to_play.id);

        // --- Update Player State Immediately ---
        // Set state *before* getting URL or reporting start
        self.current_item_id = Some(item_to_play.id.clone());
        self.position_ticks = 0;
        self.is_paused = false; // Playback starts unpaused
        self.is_playing = true;

        // --- Get Stream URL ---
        let stream_url: String = match &self.jellyfin_client { // Added type annotation
            Some(client) => {
                // Assuming get_audio_stream_url exists and returns Result<String, Error>
                match client.get_stream_url(&item_to_play.id) { // Removed .await
                    Ok(url) => url,
                    Err(e) => {
                        error!("[PLAYER] Failed to get stream URL for {}: {}", item_to_play.id, e);
                        // TODO: Maybe try next item or send error update?
                        return;
                    }
                }
            }
            None => {
                error!("[PLAYER] Jellyfin client not set, cannot get stream URL.");
                return;
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
                is_paused: self.is_paused,
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
                Err(e) => error!("[PLAYER] Failed to report playback start for item {}: {}", item_to_play.id, e),
            }
        } else {
            error!("[PLAYER] Cannot report playback start: Jellyfin client not available.");
        }

        // Send internal update for UI etc. *after* reporting start
        self.send_update(PlayerStateUpdate::Started { item: item_to_play.clone() });
        // --- Setup for new playback ---
        let progress_info = Arc::new(TokioMutex::new(PlaybackProgressInfo::default())); // Use TokioMutex
        self.current_progress = Some(progress_info.clone());

        // Ensure we have an AlsaPlayer instance
        // If not pre-set, create it here (requires device name from config/args)
        if self.alsa_player.is_none() {
             warn!("[PLAYER] AlsaPlayer not set before playback attempt.");
             // TODO: Get device name and create AlsaPlayer instance
             // let device_name = "default"; // Or from config
             // self.alsa_player = Some(Arc::new(Mutex::new(AlsaPlayer::new(device_name))));
             return; // Cannot proceed without AlsaPlayer
        }
        let alsa_player_arc = self.alsa_player.clone().unwrap(); // We checked above

        // Set the progress tracker on the AlsaPlayer instance
        { // Scope for MutexGuard
            let mut alsa_player_guard = alsa_player_arc.lock().await;
            alsa_player_guard.set_progress_tracker(progress_info.clone());
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

            // Read mutable state *before* spawning the task (will be updated by player actions)
            // We need a way to get the *current* pause/volume/mute state inside the loop.
            // Simplest approach: Use the state captured when the task starts. This might lag slightly.
            // A more complex approach involves channels or shared state for these too.
            // Let's stick with the captured state for now, similar to the previous internal update approach.
            let initial_is_paused = self.is_paused;
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
                                    volume_level: initial_volume, // Use captured state
                                    is_paused: initial_is_paused, // Use captured state
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
    }

    #[instrument(skip(self))]
    pub async fn play_pause(&mut self) {
        if self.is_playing {
            if self.is_paused {
                info!("Resumed playback");
                self.resume().await; // resume will set is_paused = false and send update
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
        if !self.is_playing || self.is_paused {
            return;
        }
        
        self.is_paused = true;
        info!("State set to paused");
        // Send progress update reflecting paused state
        if let Some(id) = self.current_item_id.clone() {
             self.send_update(PlayerStateUpdate::Progress {
                 item_id: id,
                 position_ticks: self.position_ticks,
                 is_paused: self.is_paused, // Add player state
                 volume: self.volume,       // Add player state
                 is_muted: self.is_muted,   // Add player state
             });
        }
        // TODO: Implement actual ALSA pause functionality here
    }

    #[instrument(skip(self))]
    pub async fn resume(&mut self) {
        if !self.is_playing || !self.is_paused {
            return;
        }
        
        self.is_paused = false;
        info!("State set to resumed");
        // Send progress update reflecting resumed state
        if let Some(id) = self.current_item_id.clone() {
             self.send_update(PlayerStateUpdate::Progress {
                 item_id: id,
                 position_ticks: self.position_ticks,
                 is_paused: self.is_paused, // Add player state
                 volume: self.volume,       // Add player state
                 is_muted: self.is_muted,   // Add player state
             });
        }
        // TODO: Implement actual ALSA resume functionality here
    }

    // Helper to stop playback tasks
    #[instrument(skip(self))]
    async fn stop_playback_tasks(&mut self) {
        if self.playback_task_handle.is_none() && self.reporter_task_handle.is_none() {
            // No tasks running
            return;
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

        self.current_progress = None; // Clear progress state
        info!("Playback and reporter tasks stopped.");
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
                is_paused: false, // Stopped means not paused
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
        self.stop_playback_tasks().await;

        // Update state *after* stopping tasks and reporting
        self.is_playing = false;
        self.is_paused = false;
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
        self.play_current_queue_item().await;
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
        self.play_current_queue_item().await;
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
                 is_paused: self.is_paused, // Add player state
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
        self.stop_playback_tasks().await;

        // 2. Shutdown the PlaybackOrchestrator
        if let Some(alsa_player_arc) = self.alsa_player.take() { // Take ownership
            info!("Shutting down PlaybackOrchestrator...");
            let mut player_guard = alsa_player_arc.lock().await;
            if let Err(e) = player_guard.shutdown().await {
                error!("[PLAYER] Error during PlaybackOrchestrator shutdown: {}", e);
            } else {
                info!("PlaybackOrchestrator shutdown complete.");
            }
            // Drop the guard explicitly
            drop(player_guard);
        } else {
            info!("No PlaybackOrchestrator instance to shut down.");
        }

        // 3. Clear remaining state (optional, as tasks are stopped)
        self.is_playing = false;
        self.is_paused = false;
        self.current_item_id = None;
        self.queue.clear();
        self.current_queue_index = 0;

        info!("Shutdown complete.");
    }

    }


