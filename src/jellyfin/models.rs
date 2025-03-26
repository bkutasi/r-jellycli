//! Data models for Jellyfin API responses

use serde::{Deserialize, Serialize};

/// Represents a media item in a Jellyfin library
#[derive(Deserialize, Serialize, Debug, Clone)]
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
    pub run_time_ticks: Option<i64>, // Duration in 100-nanosecond units
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
