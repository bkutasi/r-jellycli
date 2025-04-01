//! Playback-related data models for Jellyfin API

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Information for reporting playback progress
#[derive(Serialize, Debug)]
pub struct PlaybackProgressInfo {
    #[serde(rename = "ItemId")]
    pub item_id: String,
    
    #[serde(rename = "SessionId")]
    pub session_id: String,
    
    #[serde(rename = "PositionTicks")]
    pub position_ticks: i64,
    
    #[serde(rename = "IsPaused")]
    pub is_paused: bool,
    
    #[serde(rename = "IsPlaying")]
    pub is_playing: bool,
    
    #[serde(rename = "PlayMethod")]
    pub play_method: String,
    
    #[serde(rename = "RepeatMode")]
    pub repeat_mode: String,
    
    #[serde(rename = "ShuffleMode")]
    pub shuffle_mode: String,
    
    #[serde(rename = "IsMuted")]
    pub is_muted: bool,
    
    #[serde(rename = "VolumeLevel")]
    pub volume_level: i32,
    
    #[serde(rename = "AudioStreamIndex")]
    pub audio_stream_index: Option<i32>,
    
    // Add additional fields needed for proper remote control visibility
    #[serde(rename = "CanSeek")]
    pub can_seek: bool,
    
    #[serde(rename = "PlaylistItemId")]
    pub playlist_item_id: Option<String>,
    
    #[serde(rename = "PlaylistIndex")]
    pub playlist_index: Option<i32>,
    
    #[serde(rename = "PlaylistLength")]
    pub playlist_length: Option<i32>,
    
    #[serde(rename = "SubtitleStreamIndex")]
    pub subtitle_stream_index: Option<i32>,
    
    #[serde(rename = "MediaSourceId")]
    pub media_source_id: Option<String>,
}

impl PlaybackProgressInfo {
    /// Create a new playback progress info with default values
    pub fn new(item_id: String, session_id: String, position_ticks: i64, is_playing: bool, is_paused: bool) -> Self {
        PlaybackProgressInfo {
            item_id,
            session_id,
            position_ticks,
            is_paused,
            is_playing,
            play_method: "DirectPlay".to_string(),
            repeat_mode: "RepeatNone".to_string(),
            shuffle_mode: "Sorted".to_string(),
            is_muted: false,
            volume_level: 100,
            audio_stream_index: Some(0),
            can_seek: true,
            playlist_item_id: None,
            playlist_index: None,
            playlist_length: None,
            subtitle_stream_index: None,
            media_source_id: None,
        }
    }
}

/// Information for reporting playback start
#[derive(Serialize, Debug)]
pub struct PlaybackStartInfo {
    #[serde(rename = "ItemId")]
    pub item_id: String,
    
    #[serde(rename = "SessionId")]
    pub session_id: String,
    
    #[serde(rename = "PlayMethod")]
    pub play_method: String,
    
    #[serde(rename = "PlaySessionId")]
    pub play_session_id: String,
}

impl PlaybackStartInfo {
    /// Create a new playback start info with default values
    pub fn new(item_id: String, session_id: String) -> Self {
        PlaybackStartInfo {
            item_id,
            session_id: session_id.clone(),
            play_method: "DirectPlay".to_string(),
            play_session_id: session_id, // Use the same session ID for simplicity
        }
    }
}

/// Information for reporting playback stopped
#[derive(Serialize, Debug)]
pub struct PlaybackStopInfo {
    #[serde(rename = "ItemId")]
    pub item_id: String,
    
    #[serde(rename = "SessionId")]
    pub session_id: String,
    
    #[serde(rename = "PositionTicks")]
    pub position_ticks: i64,
    
    #[serde(rename = "PlaySessionId")]
    pub play_session_id: String,
}

impl PlaybackStopInfo {
    /// Create a new playback stop info
    pub fn new(item_id: String, session_id: String, position_ticks: i64) -> Self {
        PlaybackStopInfo {
            item_id,
            session_id: session_id.clone(),
            position_ticks,
            play_session_id: session_id, // Use the same session ID for simplicity
        }
    }
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
    // Arguments might be needed for Seek, etc.
    #[serde(rename = "SeekPositionTicks")]
    pub seek_position_ticks: Option<i64>,
    #[serde(rename = "ControllingUserId")]
    pub controlling_user_id: Option<String>, // Added based on potential Jellyfin structure
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
    // Add other potential fields if needed based on Jellyfin API
    #[serde(rename = "StartPositionTicks")]
    pub start_position_ticks: Option<i64>,
}
