// src/jellyfin/reporter.rs
use crate::jellyfin::api::{JellyfinApiContract, JellyfinError}; // Use trait and error from api module
// Removed Session import
use crate::jellyfin::models_playback; // Import the module itself
// Removed error import

use tracing::{debug, error, warn}; // Use tracing (removed unused info, trace)
use serde::Serialize;
// Removed unused Arc and Mutex imports

// --- Request Body Structs ---
// Based on common playback reporting fields. Refine if Jellyfin spec differs.

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct PlaybackStartInfoBody {
    pub item_id: String,
    pub queue_item_id: Option<String>, // Often needed for queue tracking
    pub can_seek: bool,
    pub is_muted: bool,
    pub is_paused: bool,
    pub volume_level: u32,
    // Add other relevant fields like PlaySessionId if needed by the API
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct PlaybackProgressInfoBody {
    pub item_id: String,
    pub queue_item_id: Option<String>,
    pub can_seek: bool,
    pub is_muted: bool,
    pub is_paused: bool,
    pub volume_level: u32,
    pub position_ticks: i64, // Typically required for progress
    pub event_name: String, // "timeupdate", "pause", "unpause" etc.
    // Add other relevant fields
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct PlaybackStopInfoBody {
    pub item_id: String,
    pub queue_item_id: Option<String>,
    pub position_ticks: i64, // Position where playback stopped
    // Add other relevant fields
}


// --- Reporting Functions ---

/// Reports that playback has started for an item.
pub async fn report_playback_start(
    api: &dyn JellyfinApiContract, // Use trait reference
    start_info: &PlaybackStartInfoBody, // Pass by reference
) -> Result<(), JellyfinError> {
    debug!("Reporting playback start: {:?}", start_info);
    // Construct the full report
    let base = models_playback::PlaybackReportBase {
        // Populate fields from start_info and api
        item_id: start_info.item_id.clone(),
        can_seek: start_info.can_seek,
        is_muted: start_info.is_muted,
        is_paused: start_info.is_paused,
        volume_level: start_info.volume_level as i32, // Convert u32 to i32
        play_session_id: api.play_session_id().to_string(),
        // --- Fields needing defaults or more context ---
        queueable_media_types: vec!["Audio".to_string()], // Default
        media_source_id: "".to_string(), // TODO: Get actual media source ID if available
        position_ticks: 0, // Start report, position is 0
        play_method: "DirectPlay".to_string(), // Default
        live_stream_id: None,
        playlist_length: 0, // TODO: Get actual item duration if available
        playlist_index: None, // TODO: Get actual queue index if available
        shuffle_mode: "Sorted".to_string(), // Default
        now_playing_queue: vec![], // TODO: Get actual queue if available
    };
    let report = models_playback::PlaybackStartReport { base };

    match api.report_playback_start(&report).await {
        Ok(_) => {
            debug!("Successfully reported playback start for item {}", start_info.item_id);
            Ok(())
        }
        Err(e) => {
            error!("Failed to report playback start for item {}: {}", start_info.item_id, e);
            Err(e)
        }
    }
}

/// Reports playback progress for an item.
pub async fn report_playback_progress(
    api: &dyn JellyfinApiContract, // Use trait reference
    progress_info: &PlaybackProgressInfoBody, // Pass by reference
) -> Result<(), JellyfinError> {
    debug!("Reporting playback progress: {:?}", progress_info);
     // Construct the full report
    let base = models_playback::PlaybackReportBase {
        // Populate fields from progress_info and api
        item_id: progress_info.item_id.clone(),
        can_seek: progress_info.can_seek,
        is_muted: progress_info.is_muted,
        is_paused: progress_info.is_paused,
        volume_level: progress_info.volume_level as i32, // Convert u32 to i32
        position_ticks: progress_info.position_ticks,
        play_session_id: api.play_session_id().to_string(),
        // --- Fields needing defaults or more context ---
        queueable_media_types: vec!["Audio".to_string()], // Default
        media_source_id: "".to_string(), // TODO: Get actual media source ID if available
        play_method: "DirectPlay".to_string(), // Default
        live_stream_id: None,
        playlist_length: 0, // TODO: Get actual item duration if available
        playlist_index: None, // TODO: Get actual queue index if available
        shuffle_mode: "Sorted".to_string(), // Default
        now_playing_queue: vec![], // TODO: Get actual queue if available
    };
    let report = models_playback::PlaybackProgressReport { base };

    match api.report_playback_progress(&report).await {
        Ok(_) => {
            // Use trace for progress reports (which are infrequent now) to avoid log spam if they become frequent again.
            // debug!("Successfully reported playback progress for item {}", progress_info.item_id);
            Ok(())
        }
        Err(e) => {
            warn!("Failed to report playback progress for item {}: {}", progress_info.item_id, e); // Warn might be better
            Err(e)
        }
    }
}

/// Reports that playback has stopped for an item.
pub async fn report_playback_stopped(
    api: &dyn JellyfinApiContract, // Use trait reference
    stop_info: &PlaybackStopInfoBody, // Pass by reference
) -> Result<(), JellyfinError> {
    debug!("Reporting playback stopped: {:?}", stop_info);
     // Construct the full report
    let base = models_playback::PlaybackReportBase {
        // Populate fields from stop_info and api
        item_id: stop_info.item_id.clone(),
        position_ticks: stop_info.position_ticks,
        play_session_id: api.play_session_id().to_string(),
        // --- Fields needing defaults or more context ---
        // These might not be strictly necessary for stop, but included for consistency
        can_seek: true, // Default
        is_muted: false, // Assume unmuted at stop? Or get last known state?
        is_paused: false, // Assume not paused at stop?
        volume_level: 100, // Assume default volume? Or get last known state?
        queueable_media_types: vec!["Audio".to_string()], // Default
        media_source_id: "".to_string(), // TODO: Get actual media source ID if available
        play_method: "DirectPlay".to_string(), // Default
        live_stream_id: None,
        playlist_length: 0, // TODO: Get actual item duration if available
        playlist_index: None, // TODO: Get actual queue index if available
        shuffle_mode: "Sorted".to_string(), // Default
        now_playing_queue: vec![], // TODO: Get actual queue if available
    };
    // Need to determine if playback completed naturally
    // For now, assume it didn't complete naturally unless explicitly told otherwise
    let stopped_info_inner = models_playback::PlaybackStoppedInfoInner {
        played_to_completion: false, // TODO: Determine this based on context if possible
    };
    let report = models_playback::PlaybackStopReport { base, playback_stopped_info: stopped_info_inner };

    match api.report_playback_stopped(&report).await {
        Ok(_) => {
            debug!("Successfully reported playback stopped for item {}", stop_info.item_id);
            Ok(())
        }
        Err(e) => {
            error!("Failed to report playback stopped for item {}: {}", stop_info.item_id, e);
            Err(e)
        }
    }
}
