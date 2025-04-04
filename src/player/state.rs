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
    Shutdown,
}

/// Represents the detailed internal state of the player.
#[derive(Debug, Clone)]
pub struct InternalPlayerState {
    pub is_playing: bool,
    pub is_paused: bool,
    pub current_item: Option<MediaItem>,
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
    Error(String),
}