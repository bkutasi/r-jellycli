//! Jellyfin API client implementation

use reqwest::{Client, Error as ReqwestError};
use std::error::Error;
use std::sync::Arc;
use uuid::Uuid;
use std::sync::atomic::{AtomicBool};
// Removed unused tokio::sync::Mutex import
// Removed unused AuthRequest import
use crate::jellyfin::models::{MediaItem, ItemsResponse, AuthResponse};
use crate::jellyfin::session::SessionManager;
use crate::jellyfin::WebSocketHandler;
use crate::player::Player;
use tokio::sync::Mutex; // Use tokio's Mutex
// Removed duplicate import: use crate::player::Player;

/// Client for interacting with Jellyfin API
#[derive(Clone)]
pub struct JellyfinClient {
    client: Client,
    server_url: String,
    api_key: Option<String>,
    user_id: Option<String>,
    session_manager: Option<SessionManager>,
    websocket_handler: Option<Arc<Mutex<WebSocketHandler>>>,
    websocket_listener_handle: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>, // Use tokio::sync::Mutex
    play_session_id: String, // Added for persistent playback session ID
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
        
        // Generate a persistent PlaySessionId for this client instance
        let play_session_id = Uuid::new_v4().to_string();
        println!("[CLIENT-DEBUG] Generated PlaySessionId: {}", play_session_id);

        JellyfinClient {
            client,
            server_url: normalized_url,
            api_key: None,
            user_id: None,
            session_manager: None,
            websocket_handler: None,
            websocket_listener_handle: Arc::new(Mutex::new(None)), // Initialize with Arc<tokio::sync::Mutex<None>>
            play_session_id, // Store the generated ID
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
    
    /// Initialize the session manager and report capabilities after authentication.
    /// This also establishes the WebSocket connection and starts the listener task.
    pub async fn initialize_session(&mut self, device_id: &str, shutdown_signal: Arc<AtomicBool>) -> Result<(), JellyfinError> {
        println!("[CLIENT-DEBUG] Initializing session...");

        if let (Some(api_key), Some(user_id)) = (&self.api_key, &self.user_id) {
            println!("[CLIENT-DEBUG] Creating session manager with user_id: {}, api_key length: {}, play_session_id: {}", user_id, api_key.len(), self.play_session_id);

            // Create the SessionManager, passing the generated play_session_id
            let session_manager = SessionManager::new(
                self.client.clone(),
                self.server_url.clone(),
                api_key.clone(),
                user_id.clone(),
                device_id.to_string(), // Pass the device_id from settings
                self.play_session_id.clone() // Pass the generated PlaySessionId
            );

            // Report capabilities to the server. This should return Ok(()) on success (204 No Content).
            match session_manager.report_capabilities().await {
                Ok(()) => {
                    println!("[CLIENT-DEBUG] Capabilities reported successfully.");
                },
                Err(e) => {
                    // Log the specific error from report_capabilities
                    println!("[CLIENT-DEBUG] Failed to report capabilities: {:?}", e);
                    // Return a more specific error if possible, otherwise wrap it
                     return Err(JellyfinError::Other(Box::new(std::io::Error::new(
                         std::io::ErrorKind::Other,
                         format!("Failed to report capabilities: {}", e),
                     ))));
                }
            };

            // Store session manager for later use (e.g., playback reporting)
            self.session_manager = Some(session_manager.clone());

            // Initialize WebSocket handler using the persistent DeviceId
            // The PlaySessionId is not needed for the WebSocket connection URL itself.
            let mut ws_handler = WebSocketHandler::new(
                self.clone(),
                &self.server_url,
                api_key,
                device_id // Use the persistent device ID from settings
            );
            // Removed .with_session_id() call as it's no longer needed/valid

            // Connect to WebSocket immediately after capability reporting
            match ws_handler.connect().await {
                Ok(()) => {
                    println!("[CLIENT-DEBUG] WebSocket connected successfully using DeviceId: {}", device_id);
                    let ws_handler_arc = Arc::new(Mutex::new(ws_handler));
                    self.websocket_handler = Some(ws_handler_arc.clone());

                    // --- Start WebSocket listener task immediately ---
                    let shutdown = shutdown_signal.clone();
                    // Clone the Arc again specifically for the task's move
                    let task_ws_handler_arc = ws_handler_arc.clone();
                    log::debug!("[WS Spawn] Attempting to spawn original listener task..."); // Log before spawn
                    let handle = tokio::spawn(async move {
                        log::trace!("[WS Listen] Original listener task started execution."); // Restore original log
                        let prepared_result = {
                            // Use the cloned Arc inside the task
                            let mut handler_guard = task_ws_handler_arc.lock().await;
                            // Ensure prepare_for_listening is called correctly
                            handler_guard.prepare_for_listening(shutdown.clone())
                        }; // MutexGuard dropped here

                        // --- Restore the missing task body ---
                        if let Some(mut prepared_handler) = prepared_result {
                            log::debug!("[WS Listen] Prepared handler obtained, starting listen_for_commands loop...");
                            if let Err(e) = prepared_handler.listen_for_commands().await {
                                log::error!("[WS Listen] Listener loop exited with error: {:?}", e);
                            } else {
                                log::debug!("[WS Listen] Listener loop exited gracefully.");
                            }
                        } else {
                            log::error!("[WS Listen] Failed to obtain prepared handler for WebSocket listener.");
                        }
                        log::debug!("[WS Listen] Listener task finished execution.");
                        // --- End of restored body ---
                    });

                    // Store the handle
                    let mut handle_guard = self.websocket_listener_handle.lock().await;
                    *handle_guard = Some(handle);
                    // --- End WebSocket listener task start ---


                    // HTTP Keep-alive pings are removed (Step 4), rely on WebSocket pings.
                    // session_manager.start_keep_alive_pings(); // Removed

                    Ok(())
                },
                Err(e) => {
                    println!("[CLIENT-DEBUG] Failed to connect to WebSocket: {:?}", e);
                    // Ensure websocket_handler is None if connection fails
                    self.websocket_handler = None;
                    Err(JellyfinError::Other(Box::new(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        format!("WebSocket connection error: {}", e)
                    ))))
                }
            }
        } else {
            println!("[CLIENT-DEBUG] Cannot initialize session - missing authentication data (API Key or User ID)");
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
            let mut handler = websocket_handler.lock().await; // Use tokio::sync::Mutex::lock (async)
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
    pub async fn set_player(&self, player: Arc<tokio::sync::Mutex<Player>>) -> Result<(), JellyfinError> { // Expect tokio::sync::Mutex
        if let Some(websocket_handler) = &self.websocket_handler {
            let mut handler = websocket_handler.lock().await; // Use tokio::sync::Mutex::lock (async)
            handler.set_player(player);
            Ok(())
        } else {
            println!("[CLIENT-DEBUG] Cannot set player - WebSocket handler not initialized");
            Err(JellyfinError::Authentication("WebSocket handler not initialized"))
        }
    }
    
    /// Start listening for WebSocket commands in a background task and store the handle
    pub fn start_websocket_listener(&mut self, shutdown_signal: Arc<AtomicBool>) -> Result<(), JellyfinError> {
        if let Some(websocket_handler) = &self.websocket_handler {
            let ws_handler = websocket_handler.clone();
            let shutdown = shutdown_signal.clone();
            
            // Spawn a background task to listen for WebSocket messages
            let handle = tokio::spawn(async move {
                println!("[CLIENT-DEBUG] Starting WebSocket listener task...");

                // Get a reference to the handler, then drop the mutex guard before awaiting
                // to prevent the Send issue with MutexGuard across .await points
                let prepared_result = {
                    let mut handler_guard = ws_handler.lock().await; // Use tokio::sync::Mutex::lock (async)
                    // Prepare the handler for listening outside the lock if possible
                    handler_guard.prepare_for_listening(shutdown.clone())
                }; // MutexGuard dropped here

                if let Some(mut prepared_handler) = prepared_result {
                    println!("[CLIENT-DEBUG] Prepared handler obtained, starting listen_for_commands loop...");
                    if let Err(e) = prepared_handler.listen_for_commands().await {
                        // Log errors from the listener loop itself
                        println!("[CLIENT-DEBUG] WebSocket listener loop exited with error: {:?}", e);
                    } else {
                        println!("[CLIENT-DEBUG] WebSocket listener loop exited gracefully.");
                    }
                } else {
                    // This case implies ws_stream was None when prepare_for_listening was called
                    println!("[CLIENT-DEBUG] Failed to prepare WebSocket handler for listening (was it connected?).");
                }
                println!("[CLIENT-DEBUG] WebSocket listener task finished execution.");
            });

            // Store the handle inside the Arc<Mutex<Option<...>>>
            // Lock synchronously from this non-async function
            let mut handle_guard = self.websocket_listener_handle.blocking_lock();
            *handle_guard = Some(handle);
            Ok(())
        } else {
            println!("[CLIENT-DEBUG] Cannot start WebSocket listener - handler not initialized");
            Err(JellyfinError::Authentication("WebSocket handler not initialized"))
        }
    }

    /// Take the JoinHandle for the WebSocket listener task, if it exists.
    /// This allows the caller to await the task's completion during shutdown.
    /// Takes the websocket listener handle out of the shared state, if present.
    pub async fn take_websocket_handle(&mut self) -> Option<tokio::task::JoinHandle<()>> {
        // Lock the mutex asynchronously and take the handle from the Option inside
        let mut handle_guard = self.websocket_listener_handle.lock().await;
        handle_guard.take()
    }
}
