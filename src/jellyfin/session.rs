//! Jellyfin session management implementation


use reqwest::{Client, Error as ReqwestError};
use tracing::debug;
use tracing::instrument;

use std::time::SystemTime;
use std::sync::{Arc, Mutex};
use serde_json;
use hostname;
use super::models::{ClientCapabilitiesDto, MediaType, GeneralCommandType};


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
    play_session_id: String,
}


impl SessionManager {
    /// Create a new session manager
    pub fn new(client: Client, server_url: String, api_key: String, user_id: String, device_id: String, play_session_id: String) -> Self {
        
        debug!(device_id = %device_id, "[SESSION] Using device ID");
        
        SessionManager {
            client,
            server_url,
            api_key,
            user_id,
            device_id,
            last_ping: Arc::new(Mutex::new(SystemTime::now())),
            play_session_id,
        }
    }

    /// Report capabilities to the Jellyfin server.
    /// This identifies the client to the server and makes it appear in the "Play On" menu.
    /// According to the reference implementation, this should return 204 No Content.
    #[instrument(skip(self), fields(device_id = %self.device_id))]
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
            "0.1.0"
        );

        debug!(url = %url, "[SESSION] Reporting capabilities");

        // Build the capabilities payload using the defined struct
        let capabilities_payload = ClientCapabilitiesDto {
            playable_media_types: Some(vec![MediaType::Audio]),
            supported_commands: Some(vec![
                // Advertise specific playback control commands as requested
                GeneralCommandType::PlayState,
                GeneralCommandType::Play,
                GeneralCommandType::SetShuffleQueue,
                GeneralCommandType::SetRepeatMode,
                GeneralCommandType::PlayNext,
            ]),
            supports_media_control: true,
            supports_persistent_identifier: false,
            device_profile: None,
            app_store_url: None,
            icon_url: None,
        };

        // Log the JSON being sent using debug level
        debug!("[SESSION] Sending capabilities JSON payload:\n{}",
            serde_json::to_string_pretty(&capabilities_payload).unwrap_or_else(|e| format!("<JSON serialization error: {}>", e))
        );

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
        debug!(status = %response.status(), "[SESSION] Server response status");

        let status = response.status();

        // Check if the status is 204 No Content, which is expected for success
        if status == reqwest::StatusCode::NO_CONTENT {
            debug!("[SESSION] Capabilities reported successfully (204 No Content).");
            Ok(())
        } else {
            // If the status is not 204, check if it's a client/server error using error_for_status()
            // This consumes the response. The '?' propagates the ReqwestError if status is 4xx/5xx.
            let successful_response = response.error_for_status()?;

            // If we reach here, the status was successful (e.g., 200 OK, 3xx) but NOT 204.
            // Log a warning and return Ok(()) as the HTTP request itself didn't fail according to reqwest.
            debug!(status = %status, "[SESSION] Warning: Unexpected successful status code. Expected 204 No Content. Treating as success for now.");
            // Consume the response body to avoid resource leaks, though we don't use it.
            let _ = successful_response.text().await;
            Ok(())
        }
    }
    
    
    
    
    /// Get the device ID
    #[allow(dead_code)]
    pub fn get_device_id(&self) -> String {
        self.device_id.clone()
    }
    
    
    
}
