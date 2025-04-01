//! Jellyfin session management implementation

use reqwest::{Client, Error as ReqwestError};
use std::time::{SystemTime, Duration};
use tokio::time;
use std::sync::{Arc, Mutex};
use serde_json;
use hostname;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::Path;
use uuid::Uuid;

use crate::jellyfin::{PlaybackProgressInfo, PlaybackStartInfo, PlaybackStopInfo};

/// Generate a unique but persistent device ID for Jellyfin
/// This mimics the approach used by jellycli which uses machineid.ProtectedID
fn generate_device_id() -> String {
    // Try to read an existing device ID from a file
    let config_dir = dirs::config_dir()
        .unwrap_or_else(|| Path::new(".").to_path_buf())
        .join("r-jellycli");
    
    let device_id_path = config_dir.join("device_id");
    
    // Check if the device ID file exists
    if device_id_path.exists() {
        // Try to read the existing device ID
        if let Ok(mut file) = File::open(&device_id_path) {
            let mut content = String::new();
            if file.read_to_string(&mut content).is_ok() && !content.is_empty() {
                return content.trim().to_string();
            }
        }
    }
    
    // If we couldn't read an existing ID, generate a new one
    // Use a combination of hostname and UUID
    let hostname = hostname::get()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_else(|_| "unknown".to_string());
    
    let uuid = Uuid::new_v4().to_string();
    let device_id = format!("jellycli-{}-{}", hostname, uuid);
    
    // Create the config directory if it doesn't exist
    if !config_dir.exists() {
        let _ = fs::create_dir_all(&config_dir);
    }
    
    // Write the new device ID to the file
    if let Ok(mut file) = File::create(&device_id_path) {
        let _ = file.write_all(device_id.as_bytes());
    }
    
    device_id
}

/// Session manager for maintaining active Jellyfin sessions
#[derive(Clone)]
#[allow(dead_code)] // Some fields are used only for future expansion
pub struct SessionManager {
    client: Client,
    server_url: String,
    api_key: String,
    user_id: String,
    device_id: String,
    session_id: Arc<Mutex<Option<String>>>,
    last_ping: Arc<Mutex<SystemTime>>,
}

/// Basic session information returned from capabilities reporting
#[derive(Debug)]
pub struct SessionInfo {
    pub session_id: String,
}

impl SessionManager {
    /// Create a new session manager
    pub fn new(client: Client, server_url: String, api_key: String, user_id: String) -> Self {
        // Generate a unique but persistent device ID that's unique to this machine
        // This is similar to jellycli's approach using machineid.ProtectedID
        let device_id = generate_device_id();
        
        println!("[SESSION] Using device ID: {}", device_id);
        
        SessionManager {
            client,
            server_url,
            api_key,
            user_id,
            device_id,
            session_id: Arc::new(Mutex::new(None)),
            last_ping: Arc::new(Mutex::new(SystemTime::now())),
        }
    }

    /// Report capabilities to server and get session ID
    pub async fn report_capabilities(&self) -> Result<SessionInfo, ReqwestError> {
        // According to Swagger API, we should use the /Sessions/Capabilities/Full endpoint
        // when sending a full JSON body of capabilities
        let url = format!("{}/Sessions/Capabilities/Full", self.server_url);
        
        // Get hostname for device name
        let hostname = hostname::get()
            .map(|h| h.to_string_lossy().to_string())
            .unwrap_or_else(|_| "rust-client".to_string());
        
        // Create auth header in the same format as jellycli
        let auth_header = format!(
            "MediaBrowser Client=\"{}\", Device=\"{}\", DeviceId=\"{}\", Version=\"{}\"",
            "r-jellycli",
            hostname::get()
                .map(|h| h.to_string_lossy().to_string())
                .unwrap_or_else(|_| "rust-client".to_string()),
            self.device_id,
            "0.1.0"
        );

        // Get current session ID if available
        let session_id = self.get_session_id();
        
        // Build URL with session ID as query param if we have one
        let url = if let Some(id) = session_id {
            format!("{}/Sessions/Capabilities/Full?id={}", self.server_url, id)
        } else {
            url
        };
        
        println!("[SESSION] Reporting full capabilities to: {}", url);
        
        // Create a simple map structure that matches jellycli's implementation
        use std::collections::HashMap;
        
        // Create inner capabilities map
        let mut capabilities = HashMap::new();
        capabilities.insert("PlayableMediaTypes", serde_json::json!(["Audio"]));
        capabilities.insert("QueueableMediaTypes", serde_json::json!(["Audio"]));
        
        // Use only valid GeneralCommandType values as documented in Jellyfin API
        capabilities.insert("SupportedCommands", serde_json::json!([
            "VolumeUp",
            "VolumeDown",
            "Mute",
            "Unmute",
            "ToggleMute",
            "SetVolume",
            "SetRepeatMode", // Changed from SetRepeat to SetRepeatMode
            "SetShuffleQueue", // Changed from SetShuffle to SetShuffleQueue
            "PreviousTrack",
            "NextTrack",
            "Play",
            "PlayPause",
            "Pause",
            "Stop",
            "Seek"
        ]));
        capabilities.insert("SupportsMediaControl", serde_json::json!(true));
        capabilities.insert("SupportsPersistentIdentifier", serde_json::json!(true)); // CRITICAL: Must be true for remote control
        capabilities.insert("ApplicationVersion", serde_json::json!("0.1.0"));
        capabilities.insert("Client", serde_json::json!("r-jellycli"));
        capabilities.insert("DeviceName", serde_json::json!(hostname.clone()));
        capabilities.insert("DeviceId", serde_json::json!(self.device_id.clone()));
        
        // Wrap capabilities in outer data map as expected by Jellyfin
        let mut data = HashMap::new();
        data.insert("capabilities", serde_json::json!(capabilities));
        
        // Log the JSON we're sending for debugging purposes
        println!("[SESSION] Sending capabilities JSON: {}", 
            serde_json::to_string_pretty(&data).unwrap_or_else(|_| String::from("<JSON serialization error>")));
            
        // Send capabilities to the server exactly like jellycli does
        let response = self.client
            .post(&url)
            .header("X-Emby-Token", &self.api_key)
            .header("X-Emby-Authorization", auth_header)
            .header("Content-Type", "application/json")
            .json(&data)
            .send()
            .await?;
            
        // Check the response status
        println!("[SESSION] Server response status: {} {}", response.status().as_u16(), response.status().canonical_reason().unwrap_or(""));
        
        // For /Sessions/Capabilities/Full, the server should respond with 204 No Content
        // when successful, which means we might need to create our own session ID
        if response.status() == reqwest::StatusCode::NO_CONTENT {
            println!("[SESSION] Server returned 204 No Content - this is expected and good!");
        }
        
        // First try to get session ID from response headers
        let mut session_id_str = None;
        
        // Try different header variations
        for header_name in &["X-Emby-Session-Id", "X-MediaBrowser-Session-Id", "X-Jellyfin-Session-Id"] {
            if let Some(session_id) = response.headers().get(*header_name) {
                if let Ok(id) = session_id.to_str() {
                    println!("[SESSION] Received session ID from header: {}", id);
                    session_id_str = Some(id.to_string());
                    break;
                }
            }
        }
        
        // If no session ID in headers and we didn't get a 204 response, try to extract from body
        if session_id_str.is_none() && response.status() != reqwest::StatusCode::NO_CONTENT {
            // Try to parse response body as JSON
            match response.text().await {
                Ok(text) => {
                    println!("[SESSION] Response body: {}", text);
                    if !text.is_empty() {
                        match serde_json::from_str::<serde_json::Value>(&text) {
                            Ok(body) => {
                                // Look for session ID in different potential locations
                                if let Some(id) = body.get("SessionId").and_then(|v| v.as_str()) {
                                    println!("[SESSION] Received session ID from body: {}", id);
                                    session_id_str = Some(id.to_string());
                                } else if let Some(id) = body.get("Id").and_then(|v| v.as_str()) {
                                    println!("[SESSION] Received session ID from body ID field: {}", id);
                                    session_id_str = Some(id.to_string());
                                }
                            }
                            Err(e) => {
                                println!("[SESSION] Failed to parse response body as JSON: {}", e);
                            }
                        }
                    } else {
                        println!("[SESSION] Response body is empty");
                    }
                }
                Err(e) => {
                    println!("[SESSION] Failed to read response body: {}", e);
                }
            }
        }
        
        // If we found a session ID, use it
        if let Some(id) = session_id_str {
            return Ok(SessionInfo {
                session_id: id,
            });
        }
        
        // If we get here, we couldn't get a session ID from the server
        println!("[SESSION] Creating a formatted session ID for remote control");
        
        // Create a properly formatted session ID using UUID
        // The format needs to match what Jellyfin expects for clients in the "play on" menu
        use uuid::Uuid;
        let session_uuid = Uuid::new_v4().to_string();
        
        // This format is critical for proper remote control visibility
        // Format: "r-jellycli_"<device_name>"_"<uuid>
        // This follows the pattern used by other clients that appear in the "play on" menu
        let hostname = hostname::get()
            .map(|h| h.to_string_lossy().to_string())
            .unwrap_or_else(|_| "rust-client".to_string());
            
        let formatted_session_id = format!("r-jellycli_{}_{}", hostname, session_uuid);
        
        println!("[SESSION] Created formatted session ID: {}", formatted_session_id);
        
        // Store the session ID in our mutex for future use
        if let Ok(mut session_id) = self.session_id.lock() {
            *session_id = Some(formatted_session_id.clone());
        }
        
        // Return the session info with our generated ID
        Ok(SessionInfo {
            session_id: formatted_session_id,
        })
    }
    
    /// Start session and keep-alive pings
    pub async fn start_session(&mut self) -> Result<(), ReqwestError> {
        // Start reporting capabilities and keep track of session
        let session_info = self.report_capabilities().await?;
        
        // Store session ID for later use
        if let Ok(mut session_id) = self.session_id.lock() {
            *session_id = Some(session_info.session_id.clone());
        }
        
        // Start background pings to keep session alive
        self.start_keep_alive_pings();
        
        Ok(())
    }
    
    /// Start background task for keep-alive pings
    pub fn start_keep_alive_pings(&self) {
        let server_url = self.server_url.clone();
        let api_key = self.api_key.clone();
        let session_id_mutex = self.session_id.clone();
        let device_id = self.device_id.clone();
        let last_ping = self.last_ping.clone();
        let _user_id = self.user_id.clone();
        
        tokio::spawn(async move {
            let mut interval = time::interval(Duration::from_secs(30)); // Ping every 30 seconds
            
            loop {
                interval.tick().await;
                
                // Get the current session ID
                let session_id = {
                    if let Ok(session_id) = session_id_mutex.lock() {
                        session_id.clone()
                    } else {
                        None
                    }
                };
                
                if let Some(id) = session_id {
                    // Create reqwest client for the ping
                    let client = reqwest::Client::new();
                    
                    // According to Swagger API, Sessions/Playing/Ping only requires playSessionId
                    let ping_url = format!("{}/Sessions/Playing/Ping", server_url);
                    
                    // Add the required playSessionId parameter as documented in the Swagger API
                    let params = [("playSessionId", id.clone())];
                    
                    println!("[SESSION] Sending session ping to: {}", ping_url);
                    
                    // Include important headers for authentication
                    match client.post(&ping_url)
                        .header("X-Emby-Token", &api_key)
                        .header("X-Emby-Authorization", format!("MediaBrowser Client=\"r-jellycli\", Device=\"r-jellycli-rust\", DeviceId=\"{}\", Version=\"0.1.0\"", device_id))
                        .query(&params) // Add the query parameters
                        .send()
                        .await {
                            Ok(response) => {
                                if response.status().is_success() {
                                    println!("[SESSION] Keep-alive ping sent successfully to {}", ping_url);
                                    // Update last ping time
                                    if let Ok(mut last) = last_ping.lock() {
                                        *last = SystemTime::now();
                                    }
                                } else {
                                    println!("[SESSION] Failed to send keep-alive ping: {} (URL: {})", response.status(), ping_url);
                                    // Try to get response body for more details
                                    match response.text().await {
                                        Ok(text) => println!("[SESSION] Response body: {}", text),
                                        Err(_) => println!("[SESSION] Could not read response body")
                                    }
                                }
                            }
                            Err(e) => {
                                println!("[SESSION] Error sending keep-alive ping: {}", e);
                            }
                        }
                }
                
                // Sleep to prevent excessive CPU usage in case of errors
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        });
    }
    
    /// Get the current session ID
    pub fn get_session_id(&self) -> Option<String> {
        let session_id = self.session_id.lock().unwrap();
        session_id.clone()
    }
    
    /// Get the device ID
    pub fn get_device_id(&self) -> String {
        self.device_id.clone()
    }
    
    /// Report playback progress to Jellyfin server
    pub async fn report_playback_progress(
        &self,
        item_id: &str,
        position_ticks: i64,
        is_playing: bool,
        is_paused: bool
    ) -> Result<(), ReqwestError> {
        // Get the current session ID
        let session_id = match self.get_session_id() {
            Some(id) => id,
            None => {
                println!("[SESSION] Warning: No session ID available, using generated ID for progress report");
                // Create a fallback session ID if none is available
                format!("r-jellycli-rust-fallback-{}", self.user_id)
            }
        };
            
        println!("[SESSION] Reporting playback progress for item: {}", item_id);
        println!("[SESSION] Position: {} ticks, Playing: {}, Paused: {}", position_ticks, is_playing, is_paused);
        
        let url = format!("{}/Sessions/Playing/Progress", self.server_url);
            
        let playback_request = PlaybackProgressInfo {
            item_id: item_id.to_string(),
            session_id,
            position_ticks,
            is_playing,
            is_paused,
            // Include required fields for remote control
            play_method: "DirectPlay".to_string(),
            repeat_mode: "RepeatNone".to_string(),
            shuffle_mode: "Sorted".to_string(),
            is_muted: false,
            volume_level: 100,
            audio_stream_index: Some(0),
            // Additional fields for remote control functionality
            can_seek: true,
            playlist_item_id: None,
            playlist_index: Some(0),
            playlist_length: Some(1),
            subtitle_stream_index: None,
            media_source_id: Some(item_id.to_string()),
        };
        
        self.client
            .post(&url)
            .header("X-Emby-Token", &self.api_key)
            .json(&playback_request)
            .send()
            .await?
            .error_for_status()?;
            
        println!("[SESSION] Playback progress reported successfully");
        Ok(())
    }
    
    /// Report playback started to Jellyfin server
    pub async fn report_playback_start(&self, item_id: &str) -> Result<(), ReqwestError> {
        if let Some(session_id) = self.get_session_id() {
            println!("[SESSION] Reporting playback start for item: {}", item_id);
            
            let url = format!("{}/Sessions/Playing", self.server_url);
            
            let start_info = PlaybackStartInfo::new(
                item_id.to_string(),
                session_id
            );
            
            self.client
                .post(&url)
                .header("X-Emby-Token", &self.api_key)
                .json(&start_info)
                .send()
                .await?
                .error_for_status()?;
                
            println!("[SESSION] Playback start reported successfully");
        } else {
            println!("[SESSION] Warning: Cannot report playback start - no active session");
        }
        
        Ok(())
    }
    
    /// Report playback stopped to Jellyfin server
    pub async fn report_playback_stopped(&self, item_id: &str, position_ticks: i64) -> Result<(), ReqwestError> {
        if let Some(session_id) = self.get_session_id() {
            println!("[SESSION] Reporting playback stopped for item: {}", item_id);
            
            let url = format!("{}/Sessions/Playing/Stopped", self.server_url);
            
            let stop_info = PlaybackStopInfo::new(
                item_id.to_string(),
                session_id,
                position_ticks
            );
            
            self.client
                .post(&url)
                .header("X-Emby-Token", &self.api_key)
                .json(&stop_info)
                .send()
                .await?
                .error_for_status()?;
                
            println!("[SESSION] Playback stopped reported successfully");
        } else {
            println!("[SESSION] Warning: Cannot report playback stopped - no active session");
        }
        
        Ok(())
    }
}
