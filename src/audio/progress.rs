use std::sync::Arc;
use tokio::sync::Mutex as TokioMutex;
use std::time::Duration as StdDuration;

pub const PROGRESS_UPDATE_INTERVAL: StdDuration = StdDuration::from_millis(500);
pub const LOG_TARGET: &str = "r_jellycli::audio::progress";

/// Holds the current playback progress information.
#[derive(Debug, Default, Clone)]
pub struct PlaybackProgressInfo {
    pub current_seconds: f64,
    pub total_seconds: Option<f64>,
}

impl PlaybackProgressInfo {
    /// Converts the current playback position from seconds to Jellyfin Ticks (10,000,000 ticks per second).
    pub fn get_position_ticks(&self) -> i64 {
        (self.current_seconds * 10_000_000.0) as i64
    }
}

pub type SharedProgress = Arc<TokioMutex<PlaybackProgressInfo>>;

// Potentially add helper functions here later if needed, e.g., for updating progress