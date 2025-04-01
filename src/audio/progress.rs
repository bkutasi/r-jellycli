use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration as StdDuration;

pub const PROGRESS_UPDATE_INTERVAL: StdDuration = StdDuration::from_millis(500);
pub const LOG_TARGET: &str = "r_jellycli::audio::progress"; // Specific log target

/// Holds the current playback progress information.
#[derive(Debug, Default, Clone)]
pub struct PlaybackProgressInfo {
    pub current_seconds: f64,
    pub total_seconds: Option<f64>,
}

// Type alias for the shared progress tracker
pub type SharedProgress = Arc<StdMutex<PlaybackProgressInfo>>;

// Potentially add helper functions here later if needed, e.g., for updating progress