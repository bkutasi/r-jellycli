//! Jellyfin session management implementation

use reqwest::{Client, Error as ReqwestError};
use std::time::{SystemTime, Duration};
use tokio::time;
use serde::Serialize;
use std::sync::{Arc, Mutex};
use serde_json;

use crate::jellyfin::{PlaybackProgressInfo, PlaybackStartInfo, PlaybackStopInfo};

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
        // Use a fixed device ID for persistent identification with Jellyfin
        let device_id = "r-jellycli-rust".to_string();
        
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
        let url = format!("{}/Sessions/Capabilities/Full", self.server_url);
        
        // Basic capabilities to identify as an audio player
        #[derive(Serialize)]
        struct Capabilities {
            #[serde(rename = "PlayableMediaTypes")]
            playable_media_types: Vec<String>,
            #[serde(rename = "SupportedCommands")]
            supported_commands: Vec<String>,
            #[serde(rename = "SupportsMediaControl")]
            supports_media_control: bool,
            #[serde(rename = "SupportsRemoteControl")]
            supports_remote_control: bool,
            #[serde(rename = "AppStoreUrl")]
            app_store_url: String,
            #[serde(rename = "IconUrl")]
            icon_url: String,
            #[serde(rename = "App")]
            app: Option<AppInfo>,
            #[serde(rename = "DeviceProfile")]
            device_profile: DeviceProfile,
            // Add device info for proper remote control identification
            #[serde(rename = "DeviceId")]
            device_id: String,
            #[serde(rename = "DeviceName")]
            device_name: String,
            #[serde(rename = "AppName")]
            app_name: String,
            #[serde(rename = "AppVersion")]
            app_version: String,
        }
        
        #[derive(Serialize)]
        struct DeviceProfile {
            #[serde(rename = "MaxStreamingBitrate")]
            max_streaming_bitrate: u64,
            #[serde(rename = "MusicStreamingTranscodingBitrate")]
            music_streaming_transcoding_bitrate: u64,
            #[serde(rename = "DirectPlayProfiles")]
            direct_play_profiles: Vec<DirectPlayProfile>,
        }
        
        #[derive(Serialize)]
        struct DirectPlayProfile {
            #[serde(rename = "Container")]
            container: String,
            #[serde(rename = "Type")]
            profile_type: String,
            #[serde(rename = "AudioCodec")]
            audio_codec: String,
        }
        
        #[derive(Serialize)]
        struct AppInfo {
            #[serde(rename = "Name")]
            name: String,
            #[serde(rename = "Version")]
            version: String,
        }

        let capabilities = Capabilities {
            playable_media_types: vec!["Audio".to_string()],
            supported_commands: vec![
                "Play".to_string(),
                "Pause".to_string(),
                "Stop".to_string(),
                "VolumeUp".to_string(),
                "VolumeDown".to_string(),
                "NextTrack".to_string(),
                "PreviousTrack".to_string(),
                "SetAudioStreamIndex".to_string(),
                "Seek".to_string(),
                "Mute".to_string(),
                "Unmute".to_string(),
                "SetVolume".to_string(),
            ],
            supports_media_control: true,
            supports_remote_control: true,
            app_store_url: "https://github.com/YOUR_GITHUB_USERNAME/r-jellycli".to_string(),
            icon_url: "".to_string(),
            app: Some(AppInfo {
                name: "r-jellycli".to_string(),
                version: "0.1.0".to_string(),
            }),
            device_profile: DeviceProfile {
                max_streaming_bitrate: 140000000,
                music_streaming_transcoding_bitrate: 1920000,
                direct_play_profiles: vec![
                    DirectPlayProfile {
                        container: "mp3,flac,ogg,wav,m4a,m4b".to_string(),
                        profile_type: "Audio".to_string(),
                        audio_codec: "mp3,flac,ogg,aac,wav".to_string(),
                    },
                ],
            },
            // Add explicit device information for Jellyfin to recognize us for remote control
            device_id: self.device_id.clone(),
            device_name: "r-jellycli Headless Player".to_string(),
            app_name: "r-jellycli".to_string(),
            app_version: "0.1.0".to_string(),
        };
        
        // Send the capabilities to the server
        let response = self.client
            .post(&url)
            .header("X-Emby-Token", &self.api_key)
            .json(&capabilities)
            .send()
            .await?;
            
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
        
        // If no session ID in headers, try to parse it from the response body
        if session_id_str.is_none() {
            // Try to parse response body as JSON
            match response.json::<serde_json::Value>().await {
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
        }
        
        // If we found a session ID, use it
        if let Some(id) = session_id_str {
            return Ok(SessionInfo {
                session_id: id,
            });
        }
        
        // If we get here, we couldn't get a session ID from the server
        // Format a session ID ourselves in the format Jellyfin expects: DeviceId-UserId-SessionId
        println!("[SESSION] Warning: No session ID received from server, creating a formatted one");
        
        // Generate a unique session component
        let session_component = uuid::Uuid::new_v4().to_string();
        
        // Create formatted session ID: DeviceId-UserId-SessionId
        let formatted_id = format!("{}-{}-{}", 
            self.device_id,
            self.user_id,
            session_component
        );
        
        println!("[SESSION] Created formatted session ID: {}", formatted_id);
        
        Ok(SessionInfo {
            session_id: formatted_id,
        })
    }
    
    /// Start session and keep-alive pings
    pub async fn start_session(&mut self) -> Result<(), ReqwestError> {
        // Report capabilities and get session ID
        let session_info = self.report_capabilities().await?;
        
        // Store session ID
        {
            let mut session_id = self.session_id.lock().unwrap();
            *session_id = Some(session_info.session_id);
        }
        
        // Start keep-alive pings
        self.start_keep_alive_pings();
        
        Ok(())
    }
    
    /// Start background task for keep-alive pings
    fn start_keep_alive_pings(&self) {
        // Clone required values for the async task
        let client = self.client.clone();
        let server_url = self.server_url.clone();
        let api_key = self.api_key.clone();
        let session_id = self.session_id.clone();
        
        tokio::spawn(async move {
            let mut interval = time::interval(Duration::from_secs(30));
            
            loop {
                interval.tick().await;
                
                let current_session_id = {
                    // Get current session ID from mutex
                    let session_id_guard = session_id.lock().unwrap();
                    session_id_guard.clone()
                };
                
                if let Some(id) = current_session_id {
                    // Send session ping
                    let url = format!("{}/Sessions/Playing/Ping", server_url);
                    
                    match client
                        .post(&url)
                        .header("X-Emby-Token", &api_key)
                        .query(&[("sessionId", &id)])
                        .send()
                        .await 
                    {
                        Ok(_) => println!("[SESSION] Keep-alive ping sent successfully"),
                        Err(e) => println!("[SESSION] Error sending keep-alive ping: {:?}", e)
                    }
                }
            }
        });
    }
    
    /// Get the current session ID
    pub fn get_session_id(&self) -> Option<String> {
        let session_id = self.session_id.lock().unwrap();
        session_id.clone()
    }
    
    /// Report playback progress to Jellyfin server
    pub async fn report_playback_progress(
        &self,
        item_id: &str,
        position_ticks: i64,
        is_playing: bool,
        is_paused: bool
    ) -> Result<(), ReqwestError> {
        if let Some(session_id) = self.get_session_id() {
            println!("[SESSION] Reporting playback progress for item: {}", item_id);
            println!("[SESSION] Position: {} ticks, Playing: {}, Paused: {}", position_ticks, is_playing, is_paused);
            
            let url = format!("{}/Sessions/Playing/Progress", self.server_url);
            
            let progress_info = PlaybackProgressInfo::new(
                item_id.to_string(),
                session_id,
                position_ticks,
                is_playing,
                is_paused
            );
            
            self.client
                .post(&url)
                .header("X-Emby-Token", &self.api_key)
                .json(&progress_info)
                .send()
                .await?
                .error_for_status()?;
                
            println!("[SESSION] Playback progress reported successfully");
        } else {
            println!("[SESSION] Warning: Cannot report progress - no active session");
        }
        
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
