//! Playback-related data models for Jellyfin API

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// --- Outgoing Playback Reporting Structures (for HTTP POST) ---

/// Represents an item in the NowPlayingQueue.
#[derive(Serialize, Debug, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct QueueItem {
    pub id: String,
    pub playlist_item_id: String,
}

/// Base structure for playback reporting (Start, Progress, Stop).
/// Matches Go's `playbackStarted` struct used in POST requests.
#[derive(Serialize, Debug, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct PlaybackReportBase {
    pub queueable_media_types: Vec<String>, // e.g., ["Audio"]
    pub can_seek: bool,
    pub item_id: String,
    pub media_source_id: String,
    pub position_ticks: i64,
    pub volume_level: i32,
    pub is_paused: bool,
    pub is_muted: bool,
    pub play_method: String, // e.g., "DirectPlay"
    pub play_session_id: String,
    pub live_stream_id: Option<String>,
    pub playlist_length: i64, // Seems to be item duration in Go code
    pub playlist_index: Option<i32>,
    pub shuffle_mode: String, // "Shuffle" or "Sorted"
    pub now_playing_queue: Vec<QueueItem>,
}

/// Information specific to reporting playback stopped.
#[derive(Serialize, Debug, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct PlaybackStoppedInfoInner {
    pub played_to_completion: bool,
}

/// Full payload for reporting playback stopped via POST /Sessions/Playing/Stopped.
/// Matches Go's `playbackStopped` struct.
#[derive(Serialize, Debug, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct PlaybackStopReport {
    #[serde(flatten)]
    pub base: PlaybackReportBase,
    pub playback_stopped_info: PlaybackStoppedInfoInner,
}

/// Full payload for reporting playback progress via POST /Sessions/Playing/Progress.
/// Matches Go's `playbackProgress` struct (excluding the redundant 'Event' field).
#[derive(Serialize, Debug, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct PlaybackProgressReport {
     #[serde(flatten)]
    pub base: PlaybackReportBase,
}

/// Full payload for reporting playback start via POST /Sessions/Playing.
/// Uses the base structure directly, matching Go's usage.
#[derive(Serialize, Debug, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct PlaybackStartReport {
     #[serde(flatten)]
    pub base: PlaybackReportBase,
}



/// Represents the full capabilities report sent to the server.
/// POST /Sessions/Capabilities/Full
/// Matches the structure used in the Go example's `ReportCapabilities`.
#[derive(Serialize, Debug, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct CapabilitiesReport {
    pub playable_media_types: Vec<String>, // e.g., ["Audio"]
    // pub queueable_media_types: Vec<String>, // Removed based on updated spec
    pub supported_commands: Vec<String>, // Commands this client actually supports
    pub supports_media_control: bool,
    pub supports_persistent_identifier: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_profile: Option<serde_json::Value>, // Added based on updated spec (structure unknown)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_store_url: Option<String>, // Added based on updated spec
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon_url: Option<String>, // Added based on updated spec
    // Removed application_version, client, device_name, device_id based on updated spec
}

// --- Structures for convenience (might be used internally before creating reports) ---

/// Simplified info for internal state tracking or function arguments.
#[derive(Debug, Clone)]
pub struct PlaybackStateInfo {
    pub item_id: String,
    pub session_id: String, // This is the PlaySessionId for reporting
    pub position_ticks: i64,
    pub duration_ticks: i64, // Item duration
    pub is_paused: bool,
    pub is_shuffle: bool,
    pub queue_ids: Vec<String>,
    pub current_queue_index: Option<usize>, // Index within queue_ids
}



// --- Incoming WebSocket Command Structures ---

/// Represents a general command received via WebSocket (e.g., SetVolume).
#[derive(Deserialize, Debug, Clone)]
pub struct GeneralCommand {
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "Arguments")]
    pub arguments: Option<HashMap<String, serde_json::Value>>,
}

/// Represents a playback state command received via WebSocket (e.g., PlayPause, Stop).
#[derive(Deserialize, Debug, Clone)]
pub struct PlayStateCommand {
    #[serde(rename = "Command")]
    pub command: String,
    #[serde(rename = "ControllingUserId")]
    pub controlling_user_id: Option<String>,
}

/// Represents a command to initiate playback received via WebSocket (e.g., PlayNow).
#[derive(Deserialize, Debug, Clone)]
pub struct PlayCommand {
    #[serde(rename = "PlayCommand")]
    pub play_command: String,
    #[serde(rename = "ItemIds")]
    pub item_ids: Vec<String>,
    #[serde(rename = "StartIndex")]
    pub start_index: Option<i32>,
}
