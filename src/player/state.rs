use crate::jellyfin::models::MediaItem;
use tokio::sync::oneshot;

/// Commands that can be sent to the Player task.
#[derive(Debug)]
pub enum PlayerCommand {
    PlayNow { item_ids: Vec<String>, start_index: usize },
    AddToQueue { item_ids: Vec<String> },
    ClearQueue,
    PlayPauseToggle,
    Next,
    Previous,
    GetFullState(oneshot::Sender<InternalPlayerState>),
    TrackFinished,
    SetVolume(u32),
    ToggleMute,
    Shutdown,
}

/// Represents the detailed internal state of the player.
#[derive(Debug, Clone)]
pub struct InternalPlayerState {
    pub is_playing: bool,
    pub is_paused: bool, // Already present
    pub is_muted: bool,  // Added for reporting
    pub volume_level: u32, // Added for reporting (e.g., 0-100)
    pub current_item: Option<MediaItem>, // Contains item details including ID
    pub position_ticks: i64,
    pub queue_ids: Vec<String>,
    pub current_queue_index: usize,
}

/// Updates broadcast by the Player task about its state changes.
#[derive(Debug, Clone, PartialEq)]
pub enum InternalPlayerStateUpdate {
    Playing {
        item: MediaItem,
        position_ticks: i64,
        queue_ids: Vec<String>,
        queue_index: usize,
    },
    Paused {
        item: MediaItem,
        position_ticks: i64,
        queue_ids: Vec<String>,
        queue_index: usize,
    },
    Stopped,
    Progress {
        item_id: String,
        position_ticks: i64,
    },
    QueueChanged {
        queue_ids: Vec<String>,
        current_index: usize,
    },
    VolumeChanged { // Added for reporting/UI updates
        volume_level: u32,
    },
    MuteStatusChanged { // Added for reporting/UI updates
        is_muted: bool,
    },
    Error(String),
}