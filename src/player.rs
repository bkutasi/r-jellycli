use std::sync::Arc;
use tokio::sync::Mutex;
use crate::audio::AlsaPlayer;
use crate::jellyfin::MediaItem;

// Player struct wraps the audio player and adds remote control capabilities
pub struct Player {
    alsa_player: Option<Arc<Mutex<AlsaPlayer>>>,
    current_item_id: Option<String>,
    position_ticks: i64,
    is_paused: bool,
    is_playing: bool,
    volume: i32,
    is_muted: bool,
    queue: Vec<MediaItem>,
    current_queue_index: usize,
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
        }
    }

    pub fn set_alsa_player(&mut self, alsa_player: Arc<Mutex<AlsaPlayer>>) {
        self.alsa_player = Some(alsa_player);
    }

    pub fn set_current_item(&mut self, item_id: &str) {
        self.current_item_id = Some(item_id.to_string());
        self.position_ticks = 0;
        self.is_paused = false;
        self.is_playing = true;
    }

    pub fn update_position(&mut self, position_ticks: i64) {
        self.position_ticks = position_ticks;
    }

    pub fn get_position(&self) -> i64 {
        self.position_ticks
    }

    pub async fn clear_queue(&mut self) {
        println!("[PLAYER] Clearing playback queue");
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
            println!("[PLAYER] No items to add to queue");
            return;
        }

        println!("[PLAYER] Adding {} items to the queue", items.len());
        for item in &items {
            println!("[PLAYER] - Added: {} ({})", item.name, item.id);
        }
        
        self.queue.extend(items);
    }

    pub async fn play_from_start(&mut self) {
        if self.queue.is_empty() {
            println!("[PLAYER] Cannot play, queue is empty");
            return;
        }

        self.current_queue_index = 0;
        self.play_current_queue_item().await;
    }

    async fn play_current_queue_item(&mut self) {
        if self.current_queue_index >= self.queue.len() {
            println!("[PLAYER] No item at index {}", self.current_queue_index);
            return;
        }

        // Clone the necessary info from the current item to avoid borrowing issues
        let item_id = self.queue[self.current_queue_index].id.clone();
        let item_name = self.queue[self.current_queue_index].name.clone();
        
        println!("[PLAYER] Playing: {} ({})", item_name, item_id);
        
        // Now we can call set_current_item without borrowing conflicts
        self.set_current_item(&item_id);
        
        if let Some(alsa_player) = &self.alsa_player {
            let _player_guard = alsa_player.lock().await;
            // Call play on the AlsaPlayer with appropriate stream URL
            // This would depend on your specific implementation
            // _player_guard.play(current_item.stream_url);
        }
    }

    pub async fn play_pause(&mut self) {
        if self.is_playing {
            if self.is_paused {
                self.is_paused = false;
                println!("[PLAYER] Resumed playback");
                self.resume().await;
            } else {
                self.is_paused = true;
                println!("[PLAYER] Paused playback");
                self.pause().await;
            }
        } else if !self.queue.is_empty() {
            self.is_playing = true;
            self.is_paused = false;
            println!("[PLAYER] Started playback");
            self.play_from_start().await;
        }
    }

    pub async fn pause(&mut self) {
        if !self.is_playing || self.is_paused {
            return;
        }
        
        self.is_paused = true;
        // Implement actual pause functionality here
    }

    pub async fn resume(&mut self) {
        if !self.is_playing || !self.is_paused {
            return;
        }
        
        self.is_paused = false;
        // Implement actual resume functionality here
    }

    pub async fn stop(&mut self) {
        if !self.is_playing {
            return;
        }
        
        self.is_playing = false;
        self.is_paused = false;
        // Implement actual stop functionality here
    }

    pub async fn next(&mut self) {
        if self.queue.is_empty() {
            println!("[PLAYER] Queue is empty, cannot skip to next item");
            return;
        }
        
        // Check if we're already at the end of the queue
        if self.current_queue_index >= self.queue.len() - 1 {
            println!("[PLAYER] Already at the end of the queue");
            return;
        }
        
        // Move to the next item and play it
        self.current_queue_index += 1;
        println!("[PLAYER] Skipping to next item (index: {})", self.current_queue_index);
        self.play_current_queue_item().await;
    }

    pub async fn previous(&mut self) {
        if self.queue.is_empty() {
            println!("[PLAYER] Queue is empty, cannot skip to previous item");
            return;
        }
        
        // Check if we're already at the start of the queue
        if self.current_queue_index == 0 {
            println!("[PLAYER] Already at the beginning of the queue");
            return;
        }
        
        // Move to the previous item and play it
        self.current_queue_index -= 1;
        println!("[PLAYER] Skipping to previous item (index: {})", self.current_queue_index);
        self.play_current_queue_item().await;
    }

    pub async fn set_volume(&mut self, volume: u8) {
        println!("[PLAYER] Setting volume to {}", volume);
        self.volume = volume as i32;
    }

    pub async fn toggle_mute(&mut self) {
        self.is_muted = !self.is_muted;
        println!("[PLAYER] Mute: {}", self.is_muted);
    }

    pub async fn seek(&mut self, position_ticks: i64) {
        println!("[PLAYER] Seeking to position: {}", position_ticks);
        self.position_ticks = position_ticks;
    }
}
