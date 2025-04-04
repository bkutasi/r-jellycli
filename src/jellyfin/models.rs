//! Data models for Jellyfin API responses

use serde::{Deserialize, Serialize};

/// Represents a media item in a Jellyfin library
#[derive(Deserialize, Serialize, Debug, Clone, PartialEq)]
pub struct MediaItem {
    #[serde(rename = "Id")]
    pub id: String,
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "Type")]
    pub media_type: String,
    #[serde(rename = "Overview", default)]
    pub overview: Option<String>,
    #[serde(rename = "Path", default)]
    pub path: Option<String>,
    #[serde(rename = "IsFolder", default)]
    pub is_folder: bool,
    #[serde(rename = "RunTimeTicks", default)]
    pub run_time_ticks: Option<i64>,
}

/// Represents a collection of media items with additional metadata
#[derive(Deserialize, Serialize, Debug)]
pub struct ItemsResponse {
    #[serde(rename = "Items")]
    pub items: Vec<MediaItem>,
    #[serde(rename = "TotalRecordCount")]
    pub total_record_count: i32,
}

/// Represents authentication request for Jellyfin
#[derive(Deserialize, Serialize, Debug)]
pub struct AuthRequest {
    #[serde(rename = "Username")]
    pub username: String,
    #[serde(rename = "PW")]
    pub pw: String,
}

/// Represents authentication response from Jellyfin
#[derive(Deserialize, Serialize, Debug)]
pub struct AuthResponse {
    #[serde(rename = "User")]
    pub user: User,
    #[serde(rename = "AccessToken")]
    pub access_token: String,
    #[serde(rename = "ServerId")]
    pub server_id: String,
}

/// Represents a user in Jellyfin
#[derive(Deserialize, Serialize, Debug)]
pub struct User {
    #[serde(rename = "Id", alias = "id")]
    pub id: String,
    #[serde(rename = "Name", alias = "name")]
    pub name: String,
    #[serde(default, rename = "ServerName", alias = "serverName")]
    pub server_name: Option<String>,
}



/// Media types supported by the client.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub enum MediaType {
    Audio,
    Video,
    Photo,
    Book,
    Unknown,
}

/// General command types supported by the client.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub enum GeneralCommandType {
    // Navigation/UI Commands (Keep existing ones)
    MoveUp,
    MoveDown,
    MoveLeft,
    MoveRight,
    PageUp,
    PageDown,
    PreviousLetter,
    NextLetter,
    ToggleOsd,
    ToggleContextMenu,
    Select,
    Back,
    TakeScreenshot,
    SendKey,
    SendString,
    GoHome,
    GoToSettings,
    ToggleFullscreen,
    DisplayContent,
    GoToSearch,
    DisplayMessage,
    ChannelUp,
    ChannelDown,
    Guide,
    ToggleStats,
    ToggleOsdMenu,

    // Playback Control Commands (Add specific ones)
    Play,
    Pause,
    Stop,
    Seek,
    NextTrack,
    PreviousTrack,
    SetVolume,
    VolumeUp,
    VolumeDown,
    Mute,
    Unmute,
    ToggleMute,
    SetAudioStreamIndex,
    SetSubtitleStreamIndex,
    SetRepeatMode,
    SetShuffleQueue,
    PlayState,
    PlayNext,

    // Other Commands (Keep existing ones)
    PlayMediaSource,
    PlayTrailers,
    SetMaxStreamingBitrate,
}

/// Represents the client capabilities sent to the server.
#[derive(Serialize, Debug, Clone)]
pub struct ClientCapabilitiesDto {
    #[serde(rename = "PlayableMediaTypes")]
    pub playable_media_types: Option<Vec<MediaType>>,
    #[serde(rename = "SupportedCommands")]
    pub supported_commands: Option<Vec<GeneralCommandType>>,
    #[serde(rename = "SupportsMediaControl")]
    pub supports_media_control: bool,
    #[serde(rename = "SupportsPersistentIdentifier")]
    pub supports_persistent_identifier: bool,
    #[serde(rename = "DeviceProfile")]
    pub device_profile: Option<serde_json::Value>,
    #[serde(rename = "AppStoreUrl")]
    pub app_store_url: Option<String>,
    #[serde(rename = "IconUrl")]
    pub icon_url: Option<String>,
}
