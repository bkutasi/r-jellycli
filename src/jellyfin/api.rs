//! Jellyfin API client implementation

use reqwest::{Client, Error as ReqwestError};
use std::error::Error;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool};
use tokio::sync::Mutex;
// Removed unused AuthRequest import
use crate::jellyfin::models::{MediaItem, ItemsResponse, AuthResponse};
use crate::jellyfin::session::SessionManager;
use crate::jellyfin::WebSocketHandler;
use crate::player::Player;

/// Client for interacting with Jellyfin API
#[derive(Clone)]
pub struct JellyfinClient {
    client: Client,
    server_url: String,
    api_key: Option<String>,
    user_id: Option<String>,
    session_manager: Option<SessionManager>,
    websocket_handler: Option<Arc<Mutex<WebSocketHandler>>>,
}

/// Error types for Jellyfin API operations
#[derive(Debug)]
pub enum JellyfinError {
    Network(ReqwestError),
    Authentication(&'static str),
    NotFound(&'static str),
    InvalidResponse(&'static str),
    Other(Box<dyn Error + Send + Sync>),
}

impl From<ReqwestError> for JellyfinError {
    fn from(err: ReqwestError) -> Self {
        JellyfinError::Network(err)
    }
}

impl From<serde_json::Error> for JellyfinError {
    fn from(_err: serde_json::Error) -> Self {
        // Use InvalidResponse since we don't have a dedicated Parse variant
        JellyfinError::InvalidResponse("Failed to parse JSON response")
    }
}

impl std::fmt::Display for JellyfinError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JellyfinError::Network(e) => write!(f, "Network error: {}", e),
            JellyfinError::Authentication(msg) => write!(f, "Authentication error: {}", msg),
            JellyfinError::NotFound(msg) => write!(f, "Not found: {}", msg),
            JellyfinError::InvalidResponse(msg) => write!(f, "Invalid response: {}", msg),
            JellyfinError::Other(e) => write!(f, "Error: {}", e),
        }
    }
}

impl Error for JellyfinError {}

impl JellyfinClient {
    /// Create a new Jellyfin client with the server URL
    pub fn new(server_url: &str) -> Self {
        println!("[CLIENT-DEBUG] Creating new JellyfinClient with server_url: {}", server_url);
        
        // Create HTTP client with extended timeout, matching the auth_test configuration
        let client = match Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build() {
                Ok(client) => {
                    println!("[CLIENT-DEBUG] HTTP client created successfully with 30s timeout");
                    client
                },
                Err(e) => {
                    println!("[CLIENT-DEBUG] Error creating HTTP client with timeout: {:?}", e);
                    println!("[CLIENT-DEBUG] Falling back to default client");
                    Client::new()
                }
            };
            
        let normalized_url = server_url.trim_end_matches('/').to_string();
        println!("[CLIENT-DEBUG] Normalized server URL: {}", normalized_url);
        
        JellyfinClient {
            client,
            server_url: normalized_url,
            api_key: None,
            user_id: None,
            session_manager: None,
            websocket_handler: None,
        }
    }

    /// Set API key for authentication
    pub fn with_api_key(mut self, api_key: &str) -> Self {
        self.api_key = Some(api_key.to_string());
        self
    }

    /// Set user ID for requests
    pub fn with_user_id(mut self, user_id: &str) -> Self {
        self.user_id = Some(user_id.to_string());
        self
    }
    
    /// Get server URL (primarily for testing)
    pub fn get_server_url(&self) -> &str {
        &self.server_url
    }
    
    /// Get API key (primarily for testing)
    pub fn get_api_key(&self) -> &Option<String> {
        &self.api_key
    }
    
    /// Get user ID (primarily for testing)
    pub fn get_user_id(&self) -> &Option<String> {
        &self.user_id
    }
    
    /// Initialize the session manager after authentication
    pub async fn initialize_session(&mut self) -> Result<(), JellyfinError> {
        println!("[CLIENT-DEBUG] Initializing session...");
        
        if let (Some(api_key), Some(user_id)) = (&self.api_key, &self.user_id) {
            println!("[CLIENT-DEBUG] Creating session manager with user_id: {} and api_key length: {}", user_id, api_key.len());
            
            let mut session_manager = SessionManager::new(
                self.client.clone(),
                self.server_url.clone(),
                api_key.clone(),
                user_id.clone()
            );
            
            // Start session
            match session_manager.start_session().await {
                Ok(()) => {
                    println!("[CLIENT-DEBUG] Session started successfully");
                    self.session_manager = Some(session_manager);
                    
                    // Initialize WebSocket handler
                    let ws_handler = WebSocketHandler::new(
                        self.clone(), // Pass the full client
                        &self.server_url,
                        &api_key,
                        "r-jellycli-rust" // Use the consistent device ID
                    );
                    self.websocket_handler = Some(Arc::new(Mutex::new(ws_handler)));
                    
                    Ok(())
                },
                Err(e) => {
                    println!("[CLIENT-DEBUG] Failed to start session: {:?}", e);
                    Err(JellyfinError::Network(e))
                }
            }
        } else {
            println!("[CLIENT-DEBUG] Cannot initialize session - missing authentication data");
            Err(JellyfinError::Authentication("Cannot initialize session without authentication"))
        }
    }
    
    /// Authenticate with Jellyfin using username and password
    pub async fn authenticate(&mut self, username: &str, password: &str) -> Result<AuthResponse, JellyfinError> {
        println!("[CLIENT-DEBUG] Authenticating with username: {}", username);
        println!("[CLIENT-DEBUG] Password length: {}", password.len());
        println!("[CLIENT-DEBUG] Server URL: {}", self.server_url);
        
        // Create a direct request using reqwest to test connectivity before calling authenticate
        println!("[CLIENT-DEBUG] Testing direct connectivity to server...");
        match self.client.get(&self.server_url).send().await {
            Ok(response) => {
                println!("[CLIENT-DEBUG] Direct connection test: {} {}", response.status(), response.status().as_str());
            },
            Err(e) => {
                println!("[CLIENT-DEBUG] Direct connection test failed: {:?}", e);
            }
        }
        
        // Use the auth module's authenticate function which is known to work
        println!("[CLIENT-DEBUG] Calling auth::authenticate function...");
        match crate::jellyfin::authenticate(&self.client, &self.server_url, username, password).await {
            Ok(auth_response) => {
                println!("[CLIENT-DEBUG] Authentication successful");
                // Store the auth info in our client
                self.api_key = Some(auth_response.access_token.clone());
                self.user_id = Some(auth_response.user.id.clone());
                Ok(auth_response)
            },
            Err(e) => {
                // Preserve the original error for better debugging
                println!("[CLIENT-DEBUG] Authentication error details: {:?}", e);
                // Create a new wrapper error that implements Send + Sync
                let err_str = format!("Authentication error: {}", e);
                Err(JellyfinError::Other(Box::new(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    err_str
                ))))
            }
        }
    }

    /// Get root items from the user's library (Views)
    pub async fn get_items(&self) -> Result<Vec<MediaItem>, JellyfinError> {
        println!("[CLIENT-DEBUG] get_items (root views) called");

        // Ensure user_id and api_key are set
        let user_id = match self.user_id.as_deref() {
            Some(id) => id,
            None => {
                println!("[CLIENT-DEBUG] User ID not set before calling get_items");
                return Err(JellyfinError::Authentication("User ID not set after authentication"));
            }
        };
        let token = match self.api_key.as_deref() {
            Some(t) => t,
            None => {
                println!("[CLIENT-DEBUG] No API token available");
                return Err(JellyfinError::Authentication("API key not set"));
            }
        };

        println!("[CLIENT-DEBUG] Using user_id: {}", user_id);
        println!("[CLIENT-DEBUG] Using token: {}", token);

        // Use the /Users/{UserId}/Views endpoint to get the top-level library views
        let url = format!("{}/Users/{}/Views", self.server_url, user_id);
        println!("[CLIENT-DEBUG] Requesting URL: {}", url);

        // First get the raw response to inspect it
        let raw_response = self.client
            .get(&url)
            // No query parameter needed, user ID is in the path
            .header("X-Emby-Token", token)
            .send()
            .await?;

        println!("[CLIENT-DEBUG] Response status: {}", raw_response.status());

        // Get the response text to debug
        let response_text = raw_response.text().await?;
        println!("[CLIENT-DEBUG] Response text length: {} bytes", response_text.len());

        let response = if !response_text.is_empty() {
            println!("[CLIENT-DEBUG] First 100 chars: {}", &response_text[..std::cmp::min(100, response_text.len())]);
            // Try to parse it
            match serde_json::from_str::<ItemsResponse>(&response_text) {
                Ok(parsed) => parsed,
                Err(e) => {
                    // Log the full response text on parsing error for better debugging
                    println!("[CLIENT-DEBUG] JSON parsing error: {}. Full response text:\n{}", e, response_text);
                    return Err(JellyfinError::InvalidResponse("Failed to parse items response"));
                }
            }
        } else {
            println!("[CLIENT-DEBUG] Empty response received");
            return Err(JellyfinError::InvalidResponse("Empty response received"));
        };

        println!("[CLIENT-DEBUG] Successfully parsed response with {} items", response.items.len());
        Ok(response.items)
    }
    
    /// Get child items of a folder
    pub async fn get_items_by_parent_id(&self, parent_id: &str) -> Result<Vec<MediaItem>, JellyfinError> {
        let user_id = self.user_id.as_deref().unwrap_or("Default");
        let url = format!("{}/Users/{}/Items?ParentId={}", self.server_url, user_id, parent_id);

        let response = match &self.api_key {
            Some(token) => {
                self.client
                    .get(&url)
                    .header("X-Emby-Token", token)
                    .send()
                    .await?
                    .json::<ItemsResponse>()
                    .await?
            }
            None => return Err(JellyfinError::Authentication("API key not set")),
        };

        Ok(response.items)
    }
    
    /// Get full details for multiple items by their IDs
    pub async fn get_items_details(&self, item_ids: &[String]) -> Result<Vec<MediaItem>, JellyfinError> {
        println!("[CLIENT-DEBUG] get_items_details called for {} items", item_ids.len());

        if item_ids.is_empty() {
            return Ok(Vec::new());
        }

        let user_id = self.user_id.as_ref().ok_or(JellyfinError::Authentication("User ID not set"))?;
        let api_key = self.api_key.as_ref().ok_or(JellyfinError::Authentication("API key not set"))?;

        let ids_param = item_ids.join(",");
        let url = format!(
            "{}/Users/{}/Items?Ids={}&Fields=MediaSources,Chapters", // Added Fields for necessary data
            self.server_url,
            user_id,
            ids_param
        );

        println!("[CLIENT-DEBUG] Fetching item details from URL: {}", url);

        let response = self.client
            .get(&url)
            .header("X-Emby-Token", api_key)
            .send()
            .await
            .map_err(JellyfinError::Network)?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_else(|_| "Failed to read error body".to_string());
            println!("[CLIENT-DEBUG] Error fetching item details: Status {}, Body: {}", status, error_text);
            return Err(JellyfinError::InvalidResponse("Failed to fetch item details"));
        }

        let items_response: ItemsResponse = response.json().await.map_err(|e| {
            println!("[CLIENT-DEBUG] Error parsing item details response: {:?}", e);
            JellyfinError::InvalidResponse("Failed to parse item details response")
        })?;

        println!("[CLIENT-DEBUG] Successfully fetched details for {} items", items_response.items.len());
        Ok(items_response.items)
    }
    
    /// Get streaming URL for an item
    pub fn get_stream_url(&self, item_id: &str) -> Result<String, JellyfinError> {
        // No need to use user_id for streaming URL
        
        match &self.api_key {
            Some(token) => {
                let url = format!("{}/Audio/{}/stream?static=true&api_key={}", 
                    self.server_url, item_id, token);
                Ok(url)
            }
            None => Err(JellyfinError::Authentication("API key not set")),
        }
    }
    
    /// Report playback progress to Jellyfin server
    pub async fn report_playback_progress(
        &self,
        item_id: &str,
        position_ticks: i64,
        is_playing: bool,
        is_paused: bool
    ) -> Result<(), JellyfinError> {
        println!("[CLIENT-DEBUG] Reporting playback progress...");
        
        if let Some(session_manager) = &self.session_manager {
            match session_manager.report_playback_progress(item_id, position_ticks, is_playing, is_paused).await {
                Ok(()) => Ok(()),
                Err(e) => {
                    println!("[CLIENT-DEBUG] Error reporting playback progress: {:?}", e);
                    Err(JellyfinError::Network(e))
                }
            }
        } else {
            println!("[CLIENT-DEBUG] Cannot report progress - session manager not initialized");
            Err(JellyfinError::InvalidResponse("Session manager not initialized"))
        }
    }
    
    /// Report playback started to Jellyfin server
    pub async fn report_playback_start(&self, item_id: &str) -> Result<(), JellyfinError> {
        println!("[CLIENT-DEBUG] Reporting playback start...");
        
        if let Some(session_manager) = &self.session_manager {
            match session_manager.report_playback_start(item_id).await {
                Ok(()) => Ok(()),
                Err(e) => {
                    println!("[CLIENT-DEBUG] Error reporting playback start: {:?}", e);
                    Err(JellyfinError::Network(e))
                }
            }
        } else {
            println!("[CLIENT-DEBUG] Cannot report playback start - session manager not initialized");
            Err(JellyfinError::InvalidResponse("Session manager not initialized"))
        }
    }
    
    /// Report playback stopped to Jellyfin server
    pub async fn report_playback_stopped(&self, item_id: &str, position_ticks: i64) -> Result<(), JellyfinError> {
        println!("[CLIENT-DEBUG] Reporting playback stopped...");
        
        if let Some(session_manager) = &self.session_manager {
            match session_manager.report_playback_stopped(item_id, position_ticks).await {
                Ok(()) => Ok(()),
                Err(e) => {
                    println!("[CLIENT-DEBUG] Error reporting playback stopped: {:?}", e);
                    Err(JellyfinError::Network(e))
                }
            }
        } else {
            println!("[CLIENT-DEBUG] Cannot report playback stopped - session manager not initialized");
            Err(JellyfinError::InvalidResponse("Session manager not initialized"))
        }
    }

    /// Connect to the Jellyfin WebSocket for real-time updates and remote control
    pub async fn connect_websocket(&self) -> Result<(), JellyfinError> {
        if let Some(websocket_handler) = &self.websocket_handler {
            let mut handler = websocket_handler.lock().await;
            match handler.connect().await {
                Ok(_) => {
                    println!("[CLIENT-DEBUG] WebSocket connected successfully");
                    Ok(())
                },
                Err(e) => {
                    println!("[CLIENT-DEBUG] Failed to connect WebSocket: {:?}", e);
                    Err(JellyfinError::Other(Box::new(std::io::Error::new(
                        std::io::ErrorKind::ConnectionRefused,
                        format!("WebSocket connection failed: {}", e)
                    )) as Box<dyn Error + Send + Sync>))
                }
            }
        } else {
            println!("[CLIENT-DEBUG] Cannot connect WebSocket - handler not initialized");
            Err(JellyfinError::Authentication("WebSocket handler not initialized"))
        }
    }
    
    /// Set the player instance for the WebSocket handler to control
    pub async fn set_player(&self, player: Arc<Mutex<Player>>) -> Result<(), JellyfinError> {
        if let Some(websocket_handler) = &self.websocket_handler {
            let mut handler = websocket_handler.lock().await;
            handler.set_player(player);
            Ok(())
        } else {
            println!("[CLIENT-DEBUG] Cannot set player - WebSocket handler not initialized");
            Err(JellyfinError::Authentication("WebSocket handler not initialized"))
        }
    }
    
    /// Start listening for WebSocket commands in a background task
    pub fn start_websocket_listener(&self, shutdown_signal: Arc<AtomicBool>) -> Result<(), JellyfinError> {
        if let Some(websocket_handler) = &self.websocket_handler {
            let ws_handler = websocket_handler.clone();
            let shutdown = shutdown_signal.clone();
            
            // Spawn a background task to listen for WebSocket messages
            tokio::spawn(async move {
                println!("[CLIENT-DEBUG] Starting WebSocket listener");
                
                // Get a reference to the handler, then drop the mutex guard before awaiting
                // to prevent the Send issue with MutexGuard across .await points
                let result = {
                    let mut handler = ws_handler.lock().await;
                    // Clone anything from handler we need for the listen_for_commands call
                    handler.prepare_for_listening(shutdown.clone())
                };
                
                if let Some(mut prepared_handler) = result {
                    if let Err(e) = prepared_handler.listen_for_commands().await {
                        println!("[CLIENT-DEBUG] WebSocket listener error: {:?}", e);
                    }
                } else {
                    println!("[CLIENT-DEBUG] Failed to prepare WebSocket handler for listening");
                }
                
                println!("[CLIENT-DEBUG] WebSocket listener stopped");
            });
            
            Ok(())
        } else {
            println!("[CLIENT-DEBUG] Cannot start WebSocket listener - handler not initialized");
            Err(JellyfinError::Authentication("WebSocket handler not initialized"))
        }
    }
}
