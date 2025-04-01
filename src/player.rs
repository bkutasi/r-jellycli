use std::sync::{Arc, Mutex as StdMutex}; // Use StdMutex for shared progress
use std::time::Duration as StdDuration;
use tokio::sync::{mpsc, Mutex, broadcast}; // Added broadcast for shutdown
use tokio::task::JoinHandle;
use crate::audio::{PlaybackOrchestrator, PlaybackProgressInfo}; // Renamed AlsaPlayer
use crate::jellyfin::api::JellyfinClient; // Need client for stream URL
use crate::jellyfin::models::MediaItem;
use crate::jellyfin::websocket::PlayerStateUpdate;
use log::{debug, error, info, warn};

// Player struct wraps the audio player and adds remote control capabilities
pub struct Player {
    alsa_player: Option<Arc<Mutex<PlaybackOrchestrator>>>, // Renamed from AlsaPlayer
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
    current_progress: Option<Arc<StdMutex<PlaybackProgressInfo>>>,
    // Handles for background tasks
    playback_task_handle: Option<JoinHandle<Result<(), crate::audio::AudioError>>>,
    reporter_task_handle: Option<JoinHandle<()>>,
    // Shutdown signal for playback task
    playback_shutdown_tx: Option<broadcast::Sender<()>>,
    // Shutdown signal for reporter task
    reporter_shutdown_tx: Option<broadcast::Sender<()>>, // Added
}

impl Player {
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
        info!("[PLAYER] Jellyfin client configured.");
        self.jellyfin_client = Some(client);
    }

    // Keep set_alsa_player if needed for pre-creation, or remove if created on play
    pub fn set_alsa_player(&mut self, alsa_player: Arc<Mutex<PlaybackOrchestrator>>) { // Renamed from AlsaPlayer
        info!("[PLAYER] ALSA player instance set.");
        self.alsa_player = Some(alsa_player);
    }

    // Method to set the update sender
    pub fn set_update_sender(&mut self, tx: mpsc::UnboundedSender<PlayerStateUpdate>) {
        info!("[PLAYER] State update sender configured.");
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

    pub async fn clear_queue(&mut self) {
        info!("[PLAYER] Clearing playback queue");
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
            info!("[PLAYER] No items to add to queue");
            return;
        }

        info!("[PLAYER] Adding {} items to the queue", items.len());
        for item in &items {
            debug!("[PLAYER] - Added: {} ({})", item.name, item.id); // Use debug for item details
        }
        
        self.queue.extend(items);
    }

    pub async fn play_from_start(&mut self) {
        if self.queue.is_empty() {
            warn!("[PLAYER] Cannot play, queue is empty");
            return;
        }

        self.current_queue_index = 0;
        self.play_current_queue_item().await;
    }

    async fn play_current_queue_item(&mut self) {
        // --- Stop existing playback first ---
        self.stop_playback_tasks().await; // Stop previous tasks if any

        if self.current_queue_index >= self.queue.len() {
            warn!("[PLAYER] No item at index {} to play.", self.current_queue_index);
            self.is_playing = false; // Ensure state reflects no playback
            return;
        }

        let item_to_play = self.queue[self.current_queue_index].clone();
        info!("[PLAYER] Attempting to play: {} ({})", item_to_play.name, item_to_play.id);

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

        // --- Setup for new playback ---
        let progress_info = Arc::new(StdMutex::new(PlaybackProgressInfo::default()));
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
        info!("[PLAYER] Spawned playback task.");


        // --- Spawn Progress Reporter Task ---
        let reporter_handle = if let Some(tx) = self.update_tx.clone() {
            let weak_progress = Arc::downgrade(&progress_info); // Use weak ref to avoid cycles

            // --- Reliable State Access for Reporter ---
            // Clone necessary Arcs/senders needed inside the reporter task
            // We need a way to get current pause/volume/mute state without deadlocking
            // Option 4: WebSocketHandler fetches state when receiving Progress update (Chosen for simplicity now)

            let initial_item_id = item_to_play.id.clone();
            // Pass the receiver for this specific reporter
            let mut reporter_shutdown_rx_clone = reporter_shutdown_rx; // Clone receiver for the task

            // Read current state *before* spawning the task
            let current_is_paused = self.is_paused;
            let current_volume = self.volume;
            let current_is_muted = self.is_muted;

            let handle = tokio::spawn(async move {
                let mut interval = tokio::time::interval(StdDuration::from_secs(1)); // Report every second
                info!("[Reporter] Progress reporter task started for item {}", initial_item_id);
                loop {
                    tokio::select! {
                        biased; // Check shutdown first
                        _ = reporter_shutdown_rx_clone.recv() => {
                            info!("[Reporter] Shutdown signal received. Stopping reporter.");
                            break;
                        }
                        _ = interval.tick() => {
                            if let Some(progress_arc) = weak_progress.upgrade() {
                                let current_secs = { // Scope for lock guard
                                    match progress_arc.lock() {
                                        Ok(guard) => guard.current_seconds,
                                        Err(_) => {
                                            warn!("[Reporter] Progress mutex poisoned.");
                                            break; // Stop reporting if mutex fails
                                        }
                                    }
                                };
                                let position_ticks = (current_secs * 10_000_000.0) as i64;

                                // Send full progress state using the captured values
                                let update = PlayerStateUpdate::Progress {
                                    item_id: initial_item_id.clone(),
                                    position_ticks,
                                    is_paused: current_is_paused, // Use captured value
                                    volume: current_volume,       // Use captured value
                                    is_muted: current_is_muted,   // Use captured value
                                };
                                if tx.send(update).is_err() {
                                    warn!("[Reporter] Update channel closed. Stopping reporter.");
                                    break;
                                }
                            } else {
                                info!("[Reporter] Progress info dropped. Stopping reporter.");
                                break; // Stop if progress info is gone
                            }
                        }
                    }
                }
                info!("[Reporter] Progress reporter task stopped for item {}", initial_item_id);
            });
            Some(handle)
        } else {
            warn!("[PLAYER] Cannot start progress reporter: update sender not configured.");
            None
        };
        self.reporter_task_handle = reporter_handle;


        // --- Finalize State ---
        // Set current item *after* potentially stopping old playback and spawning new tasks
        self.set_current_item(&item_to_play); // This sends the PlaybackStart update
    }

    pub async fn play_pause(&mut self) {
        if self.is_playing {
            if self.is_paused {
                info!("[PLAYER] Resumed playback");
                self.resume().await; // resume will set is_paused = false and send update
            } else {
                info!("[PLAYER] Paused playback");
                self.pause().await;
            }
        } else if !self.queue.is_empty() {
            // This case should ideally be handled by play_from_start or similar
            // If play_pause starts playback, ensure set_current_item is called
            info!("[PLAYER] Started playback via play_pause");
            self.play_from_start().await;
        }
    }

    pub async fn pause(&mut self) {
        if !self.is_playing || self.is_paused {
            return;
        }
        
        self.is_paused = true;
        info!("[PLAYER] State set to paused");
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

    pub async fn resume(&mut self) {
        if !self.is_playing || !self.is_paused {
            return;
        }
        
        self.is_paused = false;
        info!("[PLAYER] State set to resumed");
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
    async fn stop_playback_tasks(&mut self) {
        if self.playback_task_handle.is_none() && self.reporter_task_handle.is_none() {
            // No tasks running
            return;
        }
        info!("[PLAYER] Stopping playback and reporter tasks...");

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
                 Err(e) if e.is_cancelled() => info!("[PLAYER] Reporter task cancelled."),
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
                Err(e) if e.is_cancelled() => info!("[PLAYER] Playback task cancelled."),
                Err(e) => error!("[PLAYER] Playback task panicked or failed: {}", e),
            }
        }

        self.current_progress = None; // Clear progress state
        info!("[PLAYER] Playback and reporter tasks stopped.");
    }


    pub async fn stop(&mut self) {
        if !self.is_playing {
            debug!("[PLAYER] Stop called but not playing.");
            return;
        }
        info!("[PLAYER] Stop requested.");

        let stopped_item_id = self.current_item_id.clone();
        // Get final position from shared state if possible, otherwise use last known
        let final_ticks = self.current_progress.as_ref()
            .and_then(|p| p.lock().ok())
            .map(|p_info| (p_info.current_seconds * 10_000_000.0) as i64)
            .unwrap_or(self.position_ticks);


        // Stop background tasks
        self.stop_playback_tasks().await;

        // Update state *after* stopping tasks
        self.is_playing = false;
        self.is_paused = false;
        self.current_item_id = None;
        self.position_ticks = 0; // Reset position after stopping

        // Send Stopped update
        if let Some(id) = stopped_item_id {
            self.send_update(PlayerStateUpdate::Stopped {
                item_id: id,
                final_position_ticks: final_ticks,
            });
        }
    }

    pub async fn next(&mut self) {
        if self.queue.is_empty() {
            warn!("[PLAYER] Queue is empty, cannot skip to next item");
            return;
        }
        
        if self.current_queue_index >= self.queue.len() - 1 {
            info!("[PLAYER] Already at the end of the queue");
            return;
        }
        
        self.current_queue_index += 1;
        info!("[PLAYER] Skipping to next item (index: {})", self.current_queue_index);
        self.play_current_queue_item().await;
    }

    pub async fn previous(&mut self) {
        if self.queue.is_empty() {
            warn!("[PLAYER] Queue is empty, cannot skip to previous item");
            return;
        }
        
        if self.current_queue_index == 0 {
            info!("[PLAYER] Already at the beginning of the queue");
            return;
        }
        
        self.current_queue_index -= 1;
        info!("[PLAYER] Skipping to previous item (index: {})", self.current_queue_index);
        self.play_current_queue_item().await;
    }

    pub async fn set_volume(&mut self, volume: u8) {
        let clamped_volume = volume.min(100); // Ensure volume is 0-100
        info!("[PLAYER] Setting volume to {}", clamped_volume);
        self.volume = clamped_volume as i32;
        // Volume changes are implicitly reported via Progress updates
        // which fetch the latest state in the WS handler.
        // No specific VolumeChanged update needed here.
        // Consider sending a Progress update if immediate feedback is desired.
        // self.send_progress_update().await; // Example if needed
        // TODO: Implement actual ALSA volume control
    }

    pub async fn toggle_mute(&mut self) {
        self.is_muted = !self.is_muted;
        info!("[PLAYER] Mute toggled: {}", self.is_muted);
        // Mute changes are implicitly reported via Progress updates
        // which fetch the latest state in the WS handler.
        // No specific VolumeChanged update needed here.
        // Consider sending a Progress update if immediate feedback is desired.
        // self.send_progress_update().await; // Example if needed
        // TODO: Implement actual ALSA mute control
    }

    pub async fn seek(&mut self, position_ticks: i64) {
        info!("[PLAYER] Seeking to position: {}", position_ticks);
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
}
