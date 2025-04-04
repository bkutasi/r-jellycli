
use std::sync::Arc;
use tracing::{error, info, trace, warn, instrument};
use crate::jellyfin::api::{JellyfinApiContract, JellyfinError};
use crate::jellyfin::models::MediaItem;

pub use crate::jellyfin::models_playback::{
    PlaybackReportBase, QueueItem, PlaybackStartReport, PlaybackStopReport,
    PlaybackStoppedInfoInner, PlaybackProgressReport,
};

const REPORTER_LOG_TARGET: &str = "r_jellycli::jellyfin::reporter";

/// Represents the state needed to build playback reports.
/// This snapshot avoids needing direct access to the Player's internal mutable state.
#[derive(Debug, Clone)]
pub struct PlaybackStateSnapshot {
    pub queue: Vec<MediaItem>,
    pub current_queue_index: usize,
    pub current_item_id: Option<String>,
    pub position_ticks: i64,
    pub is_paused: bool,
}

/// Handles reporting playback status updates to the Jellyfin server.
pub struct JellyfinReporter {
    jellyfin_client: Arc<dyn JellyfinApiContract>,
}

impl JellyfinReporter {
    pub fn new(jellyfin_client: Arc<dyn JellyfinApiContract>) -> Self {
        Self { jellyfin_client }
    }

    /// Constructs the base report structure from the current player state snapshot.
    fn build_report_base(&self, state: &PlaybackStateSnapshot) -> Option<PlaybackReportBase> {
        let item_id = state.current_item_id.clone()?;

        // Find the item in the *snapshot* queue to get its duration
        let item_duration = state.queue.iter()
            .find(|item| Some(&item.id) == state.current_item_id.as_ref())
            .and_then(|item| item.run_time_ticks)
            .unwrap_or(0);

        let queue_items: Vec<QueueItem> = state.queue.iter().enumerate().map(|(idx, i)| QueueItem {
            id: i.id.clone(),
            playlist_item_id: format!("playlistItem{}", idx),
        }).collect();

        let session_id = self.jellyfin_client.play_session_id().to_string();

        Some(PlaybackReportBase {
            queueable_media_types: vec!["Audio".to_string()],
            can_seek: false,
            item_id: item_id.clone(),
            media_source_id: item_id.clone(),
            position_ticks: state.position_ticks,
            volume_level: 0,
            is_paused: state.is_paused,
            is_muted: false,
            play_method: "DirectPlay".to_string(),
            play_session_id: session_id,
            live_stream_id: None,
            playlist_length: item_duration,
            playlist_index: Some(state.current_queue_index as i32),
            shuffle_mode: "Sorted".to_string(),
            now_playing_queue: queue_items,
        })
    }

    /// Reports playback start via HTTP POST.
    #[instrument(skip(self, state), fields(item_id = %state.current_item_id.clone().unwrap_or_default()))]
    pub async fn report_playback_start(&self, state: &PlaybackStateSnapshot) {
        let base = match self.build_report_base(state) {
            Some(mut base) => {
                base.position_ticks = 0;
                base.is_paused = false;
                base
            },
            None => {
                warn!(target: REPORTER_LOG_TARGET, "Cannot report playback start: Missing current item ID in state snapshot.");
                return;
            }
        };

        let start_report = PlaybackStartReport { base };

        match self.jellyfin_client.report_playback_start(&start_report).await {
            Ok(_) => info!(target: REPORTER_LOG_TARGET, "Reported playback start successfully."),
            Err(e) => error!(target: REPORTER_LOG_TARGET, "Failed to report playback start: {}", e),
        }
    }

    /// Reports playback stop via HTTP POST.
    #[instrument(skip(self, state, completed), fields(item_id = %state.current_item_id.clone().unwrap_or_default(), completed))]
    pub async fn report_playback_stop(&self, state: &PlaybackStateSnapshot, completed: bool) {
         let base = match self.build_report_base(state) {
            Some(mut base) => {
                // Ensure stop report reflects final state correctly
                base.is_paused = false;
                base
            },
            None => {
                // If stop is called without a current_item_id (e.g., ClearQueue on empty), log and exit gracefully.
                warn!(target: REPORTER_LOG_TARGET, "Cannot report playback stop: Missing current item ID in state snapshot.");
                return;
            }
        };

        let stop_report = PlaybackStopReport {
            base,
            playback_stopped_info: PlaybackStoppedInfoInner {
                played_to_completion: completed,
            },
        };

        match self.jellyfin_client.report_playback_stopped(&stop_report).await {
            Ok(_) => info!(target: REPORTER_LOG_TARGET, "Reported playback stop successfully (completed: {}).", completed),
            Err(e) => error!(target: REPORTER_LOG_TARGET, "Failed to report playback stop: {}", e),
        }
    }

     /// Reports playback progress via HTTP POST.
    #[instrument(skip(self, state), fields(item_id = %state.current_item_id.clone().unwrap_or_default()))]
    pub async fn report_playback_progress(&self, state: &PlaybackStateSnapshot) {
        let base = match self.build_report_base(state) {
            Some(base) => base,
            None => {
                trace!(target: REPORTER_LOG_TARGET, "Skipping progress report: Missing current item ID in state snapshot.");
                return;
            }
        };

        let progress_report = PlaybackProgressReport { base };

        match self.jellyfin_client.report_playback_progress(&progress_report).await {
            Ok(_) => trace!(target: REPORTER_LOG_TARGET, "Reported playback progress successfully."),
            Err(JellyfinError::Network(e)) if e.is_timeout() => {
                warn!(target: REPORTER_LOG_TARGET, "Timeout reporting progress: {}", e);
            }
            Err(e) => {
                error!(target: REPORTER_LOG_TARGET, "Failed to report playback progress: {}", e);
            }
        }
    }
}