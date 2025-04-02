//! Jellyfin API client implementation

use crate::jellyfin::{PlaybackStartReport, PlaybackProgressReport, PlaybackStopReport};
use serde::Serialize;

use crate::jellyfin::models::{ItemsResponse, MediaItem, AuthResponse};
use crate::jellyfin::session::SessionManager;
use crate::jellyfin::WebSocketHandler;
use crate::player::Player;
use reqwest::{Client, Error as ReqwestError, Response, StatusCode};
use serde::de::DeserializeOwned;
use std::error::Error;
use std::fmt;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use uuid::Uuid;

/// Client for interacting with Jellyfin API
#[derive(Clone)]
pub struct JellyfinClient {
    client: Client,
    server_url: String,
    api_key: Option<String>,
    user_id: Option<String>,
    session_manager: Option<SessionManager>,
    websocket_handler: Option<Arc<Mutex<WebSocketHandler>>>,
    websocket_listener_handle: Arc<Mutex<Option<JoinHandle<()>>>>,
    play_session_id: String,
}

/// Error types for Jellyfin API operations
#[derive(Debug)]
pub enum JellyfinError {
    Network(ReqwestError),
    Authentication(String),
    NotFound(String),
    InvalidResponse(String),
    WebSocketError(String),
    Other(String), // Simplified Other to hold a String description
}

// --- Error Implementations ---

impl fmt::Display for JellyfinError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            JellyfinError::Network(e) => write!(f, "Network error: {}", e),
            JellyfinError::Authentication(msg) => write!(f, "Authentication error: {}", msg),
            JellyfinError::NotFound(msg) => write!(f, "Not found: {}", msg),
            JellyfinError::InvalidResponse(msg) => write!(f, "Invalid response: {}", msg),
            JellyfinError::WebSocketError(msg) => write!(f, "WebSocket error: {}", msg),
            JellyfinError::Other(msg) => write!(f, "Error: {}", msg),
        }
    }
}

impl Error for JellyfinError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            JellyfinError::Network(e) => Some(e),
            _ => None,
        }
    }
}

impl From<ReqwestError> for JellyfinError {
    fn from(err: ReqwestError) -> Self {
        JellyfinError::Network(err)
    }
}

// Helper to convert generic errors to JellyfinError::Other
fn _other_error<E: Error + Send + Sync + 'static>(context: &str, err: E) -> JellyfinError {
    JellyfinError::Other(format!("{}: {}", context, err))
}

// --- JellyfinClient Implementation ---

impl JellyfinClient {
    /// Create a new Jellyfin client with the server URL
    pub fn new(server_url: &str) -> Self {
        log::debug!("Creating new JellyfinClient with server_url: {}", server_url);

        let client = match Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
        {
            Ok(client) => {
                log::debug!("HTTP client created successfully with 30s timeout");
                client
            }
            Err(e) => {
                log::warn!("Error creating HTTP client with timeout: {:?}. Falling back to default.", e);
                Client::new()
            }
        };

        let normalized_url = server_url.trim_end_matches('/').to_string();
        log::debug!("Normalized server URL: {}", normalized_url);

        let play_session_id = Uuid::new_v4().to_string();
        log::debug!("Generated PlaySessionId: {}", play_session_id);

        JellyfinClient {
            client,
            server_url: normalized_url,
            api_key: None,
            user_id: None,
            session_manager: None,
            websocket_handler: None,
            websocket_listener_handle: Arc::new(Mutex::new(None)),
            play_session_id,
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

    /// Get the persistent PlaySessionId for this client instance.
    pub fn play_session_id(&self) -> &str {
        &self.play_session_id
    }


    // --- Private Helper Methods ---

    /// Builds a full URL for an API endpoint path.
    fn build_url(&self, path: &str) -> String {
        format!("{}{}", self.server_url, path) // Remove semicolon to return value




    }



    /// Checks if the client has authentication credentials.
    fn ensure_authenticated(&self) -> Result<(&str, &str), JellyfinError> {
        let api_key = self.api_key.as_deref().ok_or_else(|| JellyfinError::Authentication("API key not set".to_string()))?;
        let user_id = self.user_id.as_deref().ok_or_else(|| JellyfinError::Authentication("User ID not set".to_string()))?;
        Ok((api_key, user_id))
    }

    /// Sends a GET request and deserializes the JSON response.
    async fn _get_json<T: DeserializeOwned>(&self, path: &str, query_params: Option<&[(&str, &str)]>) -> Result<T, JellyfinError> {
        let (api_key, _) = self.ensure_authenticated()?;
        let url = self.build_url(path);
        log::debug!("Sending GET request to: {}", url);

        let mut request_builder = self.client.get(&url).header("X-Emby-Token", api_key);
        if let Some(params) = query_params {
            request_builder = request_builder.query(params);
        }

        let response = request_builder.send().await?;
        Self::_handle_response(response).await
    }

    /// Sends a POST request with an empty body and expects a specific success status.
    async fn _post_empty(&self, path: &str, expected_status: StatusCode) -> Result<(), JellyfinError> {
        let (api_key, _) = self.ensure_authenticated()?;
        let url = self.build_url(path);
        log::debug!("Sending empty POST request to: {}", url);

        let response = self.client
            .post(&url)
            .header("X-Emby-Token", api_key)
            .send()
            .await?;

        if response.status() == expected_status {
            log::debug!("Empty POST request successful with status: {}", expected_status);
            Ok(())
        } else {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_else(|_| "Failed to read error body".to_string());
            log::error!("Empty POST request failed. Status: {}, Body: {}", status, error_text);
            Err(JellyfinError::InvalidResponse(format!(
                "Unexpected status code {} (expected {}). Body: {}",
                status, expected_status, error_text
            )))
        }
    }

    /// Sends a POST request with a JSON body and expects a 204 No Content on success.
    async fn _post_json_no_content<T: Serialize>(&self, path: &str, body: &T) -> Result<(), JellyfinError> {
        let (api_key, _) = self.ensure_authenticated()?;
        let url = self.build_url(path);
        log::debug!("Sending POST request with JSON body to: {}", url);

        let response = self.client
            .post(&url)
            .header("X-Emby-Token", api_key)
            .json(body) // Automatically sets Content-Type: application/json
            .send()
            .await?;

        let status = response.status();
        if status == StatusCode::NO_CONTENT {
            log::debug!("POST request successful with status: {}", status);
            Ok(())
        } else {
            let error_text = response.text().await.unwrap_or_else(|_| "Failed to read error body".to_string());
            log::error!("POST request failed. Status: {}, Body: {}", status, error_text);
             match status {
                StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => Err(JellyfinError::Authentication(format!("Authentication failed ({}): {}", status, error_text))),
                StatusCode::NOT_FOUND => Err(JellyfinError::NotFound(format!("Endpoint not found ({}): {}", status, error_text))),
                _ => Err(JellyfinError::InvalidResponse(format!(
                    "Unexpected status code {} (expected 204 No Content). Body: {}",
                    status, error_text
                ))),
            }
        }
    }
    /// Handles response status checking and JSON deserialization.
    async fn _handle_response<T: DeserializeOwned>(response: Response) -> Result<T, JellyfinError> {
        let status = response.status();
        log::trace!("Response status: {}", status);

        if status.is_success() {
            let response_text = response.text().await?;
            log::trace!("Response text length: {} bytes", response_text.len());
            if response_text.is_empty() && status == StatusCode::NO_CONTENT {
                 // Handle 204 No Content specifically if T can be Default or Option
                 // This requires T to implement Default or be wrapped in Option.
                 // For simplicity here, we assume non-empty responses for success unless it's 204.
                 // A more robust solution might involve checking `std::any::TypeId::of::<T>()`
                 // or requiring a specific trait bound.
                 // If T must be derived from an empty 204, this needs adjustment.
                 log::warn!("Received 204 No Content, but expected a JSON body for type T.");
                 return Err(JellyfinError::InvalidResponse("Received 204 No Content, but expected JSON body".to_string()));
            } else if response_text.is_empty() {
                 log::error!("Received empty response body with success status {}", status);
                 return Err(JellyfinError::InvalidResponse("Empty response body received".to_string()));
            }

            log::trace!("First 100 chars: {}", &response_text[..std::cmp::min(100, response_text.len())]);
            serde_json::from_str::<T>(&response_text).map_err(|e| {
                log::error!("JSON parsing error: {}. Full response text:\n{}", e, response_text);
                JellyfinError::InvalidResponse(format!("Failed to parse JSON response: {}", e))
            })
        } else {
            let error_text = response.text().await.unwrap_or_else(|_| "Failed to read error body".to_string());
            log::error!("Request failed. Status: {}, Body: {}", status, error_text);
            match status {
                StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => Err(JellyfinError::Authentication(format!("Authentication failed ({}): {}", status, error_text))),
                StatusCode::NOT_FOUND => Err(JellyfinError::NotFound(format!("Resource not found ({}): {}", status, error_text))),
                _ => Err(JellyfinError::InvalidResponse(format!("Request failed with status {}: {}", status, error_text))),
            }
        }
    }

    /// Reports capabilities using the SessionManager.
    async fn _report_capabilities(&self, session_manager: &SessionManager) -> Result<(), JellyfinError> {
        log::debug!("Reporting capabilities...");
        match session_manager.report_capabilities().await {
            Ok(()) => {
                log::info!("Capabilities reported successfully.");
                Ok(())
            }
            Err(e) => {
                log::error!("Failed to report capabilities: {:?}", e);
                // Assuming SessionManager::report_capabilities returns reqwest::Error or similar
                Err(JellyfinError::Other(format!("Failed to report capabilities: {}", e)))
            }
        }
    }

    /// Initializes the WebSocket handler, connects, and starts the listener task.
    async fn _initialize_websocket(
        &mut self,
        api_key: &str,
        device_id: &str,
        _shutdown_signal: Arc<AtomicBool>, // Prefixed as unused in this specific function
    ) -> Result<(), JellyfinError> {
        log::debug!("Initializing WebSocket handler with DeviceId: {}", device_id);

        let mut ws_handler = WebSocketHandler::new(
            self.clone(), // Clone the client for the handler
            &self.server_url,
            api_key,
            device_id,
        );

        match ws_handler.connect().await {
            Ok(()) => {
                log::info!("WebSocket connected successfully.");
                let ws_handler_arc = Arc::new(Mutex::new(ws_handler));
                self.websocket_handler = Some(ws_handler_arc.clone());

                // Handler created and stored, listener task will be started separately
                log::debug!("WebSocket handler created and stored. Listener task will be started later.");
                Ok(())
            }
            Err(e) => {
                log::error!("Failed to connect to WebSocket: {:?}", e);
                self.websocket_handler = None; // Ensure handler is None on failure
                Err(JellyfinError::WebSocketError(format!("WebSocket connection failed: {}", e)))
            }
        }
    }

    // --- Public API Methods ---

    /// Authenticate with Jellyfin using username and password
    pub async fn authenticate(&mut self, username: &str, password: &str) -> Result<AuthResponse, JellyfinError> {
        log::info!("Authenticating user: {}", username);
        // Removed direct connectivity test - rely on the actual auth call

        match crate::jellyfin::authenticate(&self.client, &self.server_url, username, password).await {
            Ok(auth_response) => {
                log::info!("Authentication successful for user ID: {}", auth_response.user.id);
                self.api_key = Some(auth_response.access_token.clone());
                self.user_id = Some(auth_response.user.id.clone());
                Ok(auth_response)
            }
            Err(e) => {
                log::error!("Authentication failed for user {}: {:?}", username, e);
                // Assuming authenticate returns a displayable error
                Err(JellyfinError::Authentication(format!("Authentication failed: {}", e)))
            }
        }
    }

    /// Initialize the session manager, report capabilities, and establish WebSocket connection.
    pub async fn initialize_session(&mut self, device_id: &str, shutdown_signal: Arc<AtomicBool>) -> Result<(), JellyfinError> {
        log::info!("Initializing session with DeviceId: {}", device_id);

        let (api_key, user_id) = self.ensure_authenticated()?;
        let api_key = api_key.to_string(); // Clone needed parts
        let user_id = user_id.to_string();

        log::debug!("Creating session manager for user_id: {}, play_session_id: {}", user_id, self.play_session_id);
        let session_manager = SessionManager::new(
            self.client.clone(),
            self.server_url.clone(),
            api_key.clone(),
            user_id.clone(),
            device_id.to_string(),
            self.play_session_id.clone(),
        );

        // Report capabilities first
        self._report_capabilities(&session_manager).await?;

        // Store session manager
        self.session_manager = Some(session_manager);
        log::debug!("Session manager created and stored.");

        // Initialize WebSocket
        self._initialize_websocket(&api_key, device_id, shutdown_signal).await?;

        log::info!("Session initialization complete.");
        Ok(())
    }

    /// Get root items from the user's library (Views)
    pub async fn get_items(&self) -> Result<Vec<MediaItem>, JellyfinError> {
        log::debug!("Fetching root library items (Views)");
        let (_, user_id) = self.ensure_authenticated()?;
        let path = format!("/Users/{}/Views", user_id);
        let response: ItemsResponse = self._get_json(&path, None).await?;
        log::debug!("Successfully fetched {} root items", response.items.len());
        Ok(response.items)
    }

    /// Get child items of a folder/collection
    pub async fn get_items_by_parent_id(&self, parent_id: &str) -> Result<Vec<MediaItem>, JellyfinError> {
        log::debug!("Fetching items with parent_id: {}", parent_id);
        let (_, user_id) = self.ensure_authenticated()?;
        let path = format!("/Users/{}/Items", user_id);
        let params = [("ParentId", parent_id)];
        let response: ItemsResponse = self._get_json(&path, Some(&params)).await?;
        log::debug!("Successfully fetched {} items for parent {}", response.items.len(), parent_id);
        Ok(response.items)
    }

    /// Get full details for multiple items by their IDs
    pub async fn get_items_details(&self, item_ids: &[String]) -> Result<Vec<MediaItem>, JellyfinError> {
        log::debug!("Fetching details for {} item(s)", item_ids.len());
        if item_ids.is_empty() {
            return Ok(Vec::new());
        }

        let (_, user_id) = self.ensure_authenticated()?;
        let ids_param = item_ids.join(",");
        let path = format!("/Users/{}/Items", user_id);
        // Request necessary fields for playback, etc.
        let params = [
            ("Ids", ids_param.as_str()),
            ("Fields", "MediaSources,Chapters,Overview,Genres,Studios,Artists,AlbumArtists") // Example fields
        ];

        let response: ItemsResponse = self._get_json(&path, Some(&params)).await?;
        log::debug!("Successfully fetched details for {} items", response.items.len());
        Ok(response.items)
    }

    /// Get streaming URL for an item
    pub fn get_stream_url(&self, item_id: &str) -> Result<String, JellyfinError> {
        log::debug!("Generating stream URL for item_id: {}", item_id);
        let (api_key, _) = self.ensure_authenticated()?;
        // Note: Using Audio path, adjust if video/other types are needed
        let url = format!("{}/Audio/{}/stream?static=true&api_key={}", self.server_url, item_id, api_key);
        log::debug!("Generated stream URL: {}", url);
        Ok(url)
    }

    // Removed incorrect _report_playback helper that delegated to SessionManager

    /// Report playback started to Jellyfin server via HTTP POST.
    pub async fn report_playback_start(&self, report: &PlaybackStartReport) -> Result<(), JellyfinError> {
        log::info!("Reporting playback start for item_id: {}", report.base.item_id);
        self._post_json_no_content("/Sessions/Playing", report).await
    }

    /// Report playback progress to Jellyfin server via HTTP POST.
    pub async fn report_playback_progress(&self, report: &PlaybackProgressReport) -> Result<(), JellyfinError> {
        // Avoid overly verbose logging for progress updates
        log::trace!("Reporting playback progress for item_id: {}, PositionTicks: {}", report.base.item_id, report.base.position_ticks);
        self._post_json_no_content("/Sessions/Playing/Progress", report).await
    }

    /// Report playback stopped to Jellyfin server via HTTP POST.
    pub async fn report_playback_stopped(&self, report: &PlaybackStopReport) -> Result<(), JellyfinError> {
        log::info!("Reporting playback stopped for item_id: {}", report.base.item_id);
        self._post_json_no_content("/Sessions/Playing/Stopped", report).await
    }

    /// Set the player instance for the WebSocket handler to control.
    /// This should be called *before* `start_websocket_listener`.
    pub async fn set_player(&self, player: Arc<Mutex<Player>>) -> Result<(), JellyfinError> {
        log::debug!("Attempting to set player instance for WebSocket handler");
        if let Some(websocket_handler_arc) = &self.websocket_handler {
            let mut handler_guard = websocket_handler_arc.lock().await;
            handler_guard.set_player(player);
            log::info!("Player instance successfully set on WebSocket handler.");
            Ok(())
        } else {
            log::warn!("Cannot set player - WebSocket handler not initialized or already taken.");
            Err(JellyfinError::WebSocketError("WebSocket handler not available to set player".to_string()))
        }
    }

    /// Spawns the WebSocket listener task.
    /// Requires `initialize_session` and `set_player` to have been called successfully.
    pub async fn start_websocket_listener(&mut self, shutdown_signal: Arc<AtomicBool>) -> Result<(), JellyfinError> {
        log::info!("Attempting to start WebSocket listener task...");

        let ws_handler_arc = match self.websocket_handler.clone() { // Clone Arc to move into task
            Some(arc) => arc,
            None => {
                log::error!("Cannot start listener: WebSocket handler not initialized.");
                return Err(JellyfinError::WebSocketError("WebSocket handler not initialized".to_string()));
            }
        };

        // Check if player is set before spawning (optional but good practice)
        {
            let handler_guard = ws_handler_arc.lock().await;
            if !handler_guard.is_player_set() { // Use the public accessor method
                 log::warn!("Starting WebSocket listener, but Player instance was not set beforehand. Incoming commands requiring Player will fail.");
            }
        } // Lock released


        let shutdown = shutdown_signal.clone();
        log::debug!("Spawning WebSocket listener task...");

        let handle = tokio::spawn(async move {
            log::trace!("WebSocket listener task started execution.");
            // Lock the handler Arc within the task to prepare and listen
            let prepared_result = {
                 let mut handler_guard = ws_handler_arc.lock().await;
                 // Player should be set now before prepare_for_listening is called
                 handler_guard.prepare_for_listening(shutdown.clone())
            }; // MutexGuard dropped here

            if let Some(mut prepared_handler) = prepared_result {
                log::debug!("Prepared handler obtained, starting listen_for_commands loop...");
                if let Err(e) = prepared_handler.listen_for_commands().await {
                    log::error!("WebSocket listener loop exited with error: {:?}", e);
                } else {
                    log::debug!("WebSocket listener loop exited gracefully.");
                }
            } else {
                log::error!("Failed to obtain prepared handler for WebSocket listener (was it connected?).");
            }
            log::debug!("WebSocket listener task finished execution.");
        });

        // Store the handle
        let mut handle_guard = self.websocket_listener_handle.lock().await;
        *handle_guard = Some(handle);
        log::info!("WebSocket listener task spawned and handle stored.");
        Ok(())
    }

    /// Take the JoinHandle for the WebSocket listener task, if it exists.
    /// This allows the caller to await the task's completion during shutdown.
    pub async fn take_websocket_handle(&mut self) -> Option<JoinHandle<()>> {
        log::debug!("Attempting to take WebSocket listener task handle");
        let mut handle_guard = self.websocket_listener_handle.lock().await;
        let handle = handle_guard.take();
        if handle.is_some() {
            log::debug!("WebSocket listener task handle taken");
        } else {
            log::debug!("No WebSocket listener task handle was present");
        }
        handle
    }


    // Removed duplicate play_session_id function definition. The original is at line 128.
    // --- Getter methods (primarily for testing/debugging) ---
    pub fn get_server_url(&self) -> &str { &self.server_url }
    pub fn get_api_key(&self) -> Option<&str> { self.api_key.as_deref() }
    pub fn get_user_id(&self) -> Option<&str> { self.user_id.as_deref() }
}
