//! Jellyfin session management implementation

use reqwest::{Client, Error as ReqwestError};
use log::debug;

use std::time::SystemTime; // Removed unused Duration
// Removed unused tokio::time
use std::sync::{Arc, Mutex};
use serde_json;
use hostname;

use crate::jellyfin::{PlaybackProgressInfo, PlaybackStartInfo, PlaybackStopInfo};

// Removed unused generate_device_id function.
// Device ID is now handled in main.rs and passed into SessionManager::new.

/// Session manager for maintaining active Jellyfin sessions
#[derive(Clone)]
#[allow(dead_code)] // Some fields are used only for future expansion
pub struct SessionManager {
    client: Client,
    server_url: String,
    api_key: String,
    user_id: String,
    device_id: String,
    last_ping: Arc<Mutex<SystemTime>>,
    play_session_id: String, // Added to store the persistent playback session ID
}


impl SessionManager {
    /// Create a new session manager
    pub fn new(client: Client, server_url: String, api_key: String, user_id: String, device_id: String, play_session_id: String) -> Self {
        // Use the device ID passed in from Settings, instead of generating/loading one here.
        
        println!("[SESSION] Using device ID: {}", device_id);
        
        SessionManager {
            client,
            server_url,
            api_key,
            user_id,
            device_id,
            // session_id field removed
            last_ping: Arc::new(Mutex::new(SystemTime::now())),
            play_session_id, // Store the passed-in PlaySessionId
        }
    }

    /// Report capabilities to the Jellyfin server.
    /// This identifies the client to the server and makes it appear in the "Play On" menu.
    /// According to the reference implementation, this should return 204 No Content.
    pub async fn report_capabilities(&self) -> Result<(), ReqwestError> {
        let url = format!("{}/Sessions/Capabilities/Full", self.server_url);
        
        // Get hostname for device name
        let hostname = hostname::get()
            .map(|h| h.to_string_lossy().to_string())
            .unwrap_or_else(|_| "rust-client".to_string());
        
        // Create auth header
        let auth_header = format!(
            "MediaBrowser Client=\"{}\", Device=\"{}\", DeviceId=\"{}\", Version=\"{}\"",
            "r-jellycli",
            &hostname,
            self.device_id,
            "0.1.0" // TODO: Get version from Cargo.toml
        );

        println!("[SESSION] Reporting capabilities to: {}", url);

        // Build the capabilities payload as a flat JSON object (matching Go reference)
        let capabilities_payload = serde_json::json!({
            "PlayableMediaTypes": ["Audio"],
            "QueueableMediaTypes": ["Audio"],
            "SupportedCommands": [
                "VolumeUp",
                "VolumeDown",
                "Mute",
                "Unmute",
                "ToggleMute",
                "SetVolume",
                "SetShuffleQueue"
                // Removed commands not present in Go reference for capabilities reporting
            ],
            "SupportsMediaControl": true,
            "SupportsPersistentIdentifier": false, // Set to false as per plan
            "ApplicationVersion": "0.1.0", // TODO: Get version from Cargo.toml
            "Client": "r-jellycli",
            "DeviceName": hostname,
            "DeviceId": self.device_id,
            // Removed the top-level "Capabilities" wrapper
        });

        // Log the JSON being sent using debug level
        debug!("[SESSION] Sending capabilities JSON payload:\n{}",
            serde_json::to_string_pretty(&capabilities_payload).unwrap_or_else(|e| format!("<JSON serialization error: {}>", e))
        ); // Removed trailing semicolon here

        // Send capabilities to the server
        let response = self.client
            .post(&url)
            .header("X-Emby-Token", &self.api_key)
            .header("X-Emby-Authorization", auth_header)
            .header("Content-Type", "application/json")
            .json(&capabilities_payload)
            .send()
            .await?;

        // Check the response status
        println!("[SESSION] Server response status: {} {}", response.status().as_u16(), response.status().canonical_reason().unwrap_or(""));

        let status = response.status();

        // Check if the status is 204 No Content, which is expected for success
        if status == reqwest::StatusCode::NO_CONTENT {
            println!("[SESSION] Capabilities reported successfully (204 No Content).");
            Ok(())
        } else {
            // If the status is not 204, check if it's a client/server error using error_for_status()
            // This consumes the response. The '?' propagates the ReqwestError if status is 4xx/5xx.
            let successful_response = response.error_for_status()?;

            // If we reach here, the status was successful (e.g., 200 OK, 3xx) but NOT 204.
            // Log a warning and return Ok(()) as the HTTP request itself didn't fail according to reqwest.
            println!("[SESSION] Warning: Unexpected successful status code: {}. Expected 204 No Content. Treating as success for now.", status);
            // Consume the response body to avoid resource leaks, though we don't use it.
            let _ = successful_response.text().await; // Consume the body
            Ok(())
        }
    }
    
    
    // Removed start_keep_alive_pings function as per simplification plan.
    // WebSocket Ping/Pong should handle keep-alive.
    
    
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
        // TODO: Use the generated PlaySessionId here instead of the old session_id
        // let session_id = self.play_session_id.clone(); // Placeholder for Step 3
        let session_id = self.play_session_id.clone(); // Use the stored PlaySessionId
            
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
        // TODO: Use the generated PlaySessionId here instead of the old session_id
        // let session_id = self.play_session_id.clone(); // Placeholder for Step 3
        let session_id = self.play_session_id.clone(); // Use the stored PlaySessionId

        // if let Some(session_id) = self.get_session_id() { // Original check removed
            println!("[SESSION] Reporting playback start for item: {}", item_id);

            let url = format!("{}/Sessions/Playing", self.server_url);

            let start_info = PlaybackStartInfo::new(
                item_id.to_string(),
                session_id // Use the placeholder ID
            );
        // } else {
        //     println!("[SESSION] Warning: Cannot report playback start - no active session");
        // }
            
            self.client
                .post(&url)
                .header("X-Emby-Token", &self.api_key)
                .json(&start_info)
                .send()
                .await?
                .error_for_status()?;
                
            println!("[SESSION] Playback start reported successfully");
        Ok(())
    }
    
    /// Report playback stopped to Jellyfin server
    pub async fn report_playback_stopped(&self, item_id: &str, position_ticks: i64) -> Result<(), ReqwestError> {
        // TODO: Use the generated PlaySessionId here instead of the old session_id
        // let session_id = self.play_session_id.clone(); // Placeholder for Step 3
        let session_id = self.play_session_id.clone(); // Use the stored PlaySessionId

        // if let Some(session_id) = self.get_session_id() { // Original check removed
            println!("[SESSION] Reporting playback stopped for item: {}", item_id);

            let url = format!("{}/Sessions/Playing/Stopped", self.server_url);

            let stop_info = PlaybackStopInfo::new(
                item_id.to_string(),
                session_id, // Use the placeholder ID
                position_ticks
            );
        // } else {
        //     println!("[SESSION] Warning: Cannot report playback stopped - no active session");
        // }
            
            self.client
                .post(&url)
                .header("X-Emby-Token", &self.api_key)
                .json(&stop_info)
                .send()
                .await?
                .error_for_status()?;
                
            println!("[SESSION] Playback stopped reported successfully");
        Ok(())
    }
}
