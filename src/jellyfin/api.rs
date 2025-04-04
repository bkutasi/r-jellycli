//! Jellyfin API client implementation

use crate::player::{PlayerCommand, InternalPlayerStateUpdate};
use crate::jellyfin::{PlaybackStartReport, PlaybackProgressReport, PlaybackStopReport};
use serde::Serialize;

use tracing::{debug, info, warn, error, trace}; // Added for tracing macros
use tracing::instrument;
use crate::jellyfin::models::{ItemsResponse, MediaItem, AuthResponse};
use crate::jellyfin::session::SessionManager;
use crate::jellyfin::WebSocketHandler;
// Removed unused import: use crate::player::Player;
// Removed unused import: use std::sync::atomic::AtomicBool;
use reqwest::{Client, Error as ReqwestError, Response, StatusCode};
use serde::de::DeserializeOwned;
use std::error::Error;
use std::fmt;
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc, Mutex}; // Add broadcast, Added mpsc
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
// Removed Clone derive as ReqwestError doesn't implement it
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


// --- Jellyfin API Trait for Mocking ---

#[async_trait::async_trait]
pub trait JellyfinApiContract: Send + Sync {
    // Methods required by Player and ws_incoming_handler
    async fn get_items_details(&self, item_ids: &[String]) -> Result<Vec<MediaItem>, JellyfinError>;
    async fn get_audio_stream_url(&self, item_id: &str) -> Result<String, JellyfinError>;
    async fn report_playback_start(&self, report: &PlaybackStartReport) -> Result<(), JellyfinError>;
    async fn report_playback_stopped(&self, report: &PlaybackStopReport) -> Result<(), JellyfinError>;
    async fn report_playback_progress(&self, report: &PlaybackProgressReport) -> Result<(), JellyfinError>;
    fn play_session_id(&self) -> &str;
    // Add other methods used by consumers if necessary
}

// --- JellyfinClient Implementation ---

impl JellyfinClient {
    /// Create a new Jellyfin client with the server URL
    pub fn new(server_url: &str) -> Self {
        debug!("Creating new JellyfinClient with server_url: {}", server_url);

        let client = match Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
        {
            Ok(client) => {
                debug!("HTTP client created successfully with 30s timeout");
                client
            }
            Err(e) => {
                warn!("Error creating HTTP client with timeout: {:?}. Falling back to default.", e);
                Client::new()
            }
        };

        let normalized_url = server_url.trim_end_matches('/').to_string();
        debug!("Normalized server URL: {}", normalized_url);

        let play_session_id = Uuid::new_v4().to_string();
        debug!("Generated PlaySessionId: {}", play_session_id);

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
        debug!("Sending GET request to: {}", url);

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
        debug!("Sending empty POST request to: {}", url);

        let response = self.client
            .post(&url)
            .header("X-Emby-Token", api_key)
            .send()
            .await?;

        if response.status() == expected_status {
            debug!("Empty POST request successful with status: {}", expected_status);
            Ok(())
        } else {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_else(|_| "Failed to read error body".to_string());
            error!("Empty POST request failed. Status: {}, Body: {}", status, error_text);
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
        debug!("Sending POST request with JSON body to: {}", url);

        let response = self.client
            .post(&url)
            .header("X-Emby-Token", api_key)
            .json(body) // Automatically sets Content-Type: application/json
            .send()
            .await?;

        let status = response.status();
        if status == StatusCode::NO_CONTENT {
            debug!("POST request successful with status: {}", status);
            Ok(())
        } else {
            let error_text = response.text().await.unwrap_or_else(|_| "Failed to read error body".to_string());
            error!("POST request failed. Status: {}, Body: {}", status, error_text);
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
        trace!("Response status: {}", status);

        if status.is_success() {
            let response_text = response.text().await?;
            trace!("Response text length: {} bytes", response_text.len());
            if response_text.is_empty() && status == StatusCode::NO_CONTENT {
                 // Handle 204 No Content specifically if T can be Default or Option
                 // This requires T to implement Default or be wrapped in Option.
                 // For simplicity here, we assume non-empty responses for success unless it's 204.
                 // A more robust solution might involve checking `std::any::TypeId::of::<T>()`
                 // or requiring a specific trait bound.
                 // If T must be derived from an empty 204, this needs adjustment.
                 warn!("Received 204 No Content, but expected a JSON body for type T.");
                 return Err(JellyfinError::InvalidResponse("Received 204 No Content, but expected JSON body".to_string()));
            } else if response_text.is_empty() {
                 error!("Received empty response body with success status {}", status);
                 return Err(JellyfinError::InvalidResponse("Empty response body received".to_string()));
            }

            trace!("First 100 chars: {}", &response_text[..std::cmp::min(100, response_text.len())]);
            serde_json::from_str::<T>(&response_text).map_err(|e| {
                error!("JSON parsing error: {}. Full response text:\n{}", e, response_text);
                JellyfinError::InvalidResponse(format!("Failed to parse JSON response: {}", e))
            })
        } else {
            let error_text = response.text().await.unwrap_or_else(|_| "Failed to read error body".to_string());
            error!("Request failed. Status: {}, Body: {}", status, error_text);
            match status {
                StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => Err(JellyfinError::Authentication(format!("Authentication failed ({}): {}", status, error_text))),
                StatusCode::NOT_FOUND => Err(JellyfinError::NotFound(format!("Resource not found ({}): {}", status, error_text))),
                _ => Err(JellyfinError::InvalidResponse(format!("Request failed with status {}: {}", status, error_text))),
            }
        }
    }

    /// Reports capabilities using the SessionManager.
    async fn _report_capabilities(&self, session_manager: &SessionManager) -> Result<(), JellyfinError> {
        debug!("Reporting capabilities...");
        match session_manager.report_capabilities().await {
            Ok(()) => {
                info!("Capabilities reported successfully.");
                Ok(())
            }
            Err(e) => {
                error!("Failed to report capabilities: {:?}", e);
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
        shutdown_tx: broadcast::Sender<()>, // Change parameter type
    ) -> Result<(), JellyfinError> {
        debug!("Initializing WebSocket handler with DeviceId: {}", device_id);

        let mut ws_handler = WebSocketHandler::new(
            self.clone(), // Clone the client for the handler
            &self.server_url,
            api_key,
            device_id,
            shutdown_tx, // Pass the sender
        );

        match ws_handler.connect().await {
            Ok(()) => {
                info!("WebSocket connected successfully.");
                let ws_handler_arc = Arc::new(Mutex::new(ws_handler));
                self.websocket_handler = Some(ws_handler_arc.clone());

                // Handler created and stored, listener task will be started separately
                debug!("WebSocket handler created and stored. Listener task will be started later.");
                Ok(())
            }
            Err(e) => {
                error!("Failed to connect to WebSocket: {:?}", e);
                self.websocket_handler = None; // Ensure handler is None on failure
                Err(JellyfinError::WebSocketError(format!("WebSocket connection failed: {}", e)))
            }
        }
    }

    // --- Public API Methods ---

    /// Authenticate with Jellyfin using username and password
    #[instrument(skip(self, password), fields(username))]
    pub async fn authenticate(&mut self, username: &str, password: &str) -> Result<AuthResponse, JellyfinError> {
        info!("Authenticating user: {}", username);
        // Removed direct connectivity test - rely on the actual auth call

        match crate::jellyfin::authenticate(&self.client, &self.server_url, username, password).await {
            Ok(auth_response) => {
                info!("Authentication successful for user ID: {}", auth_response.user.id);
                self.api_key = Some(auth_response.access_token.clone());
                self.user_id = Some(auth_response.user.id.clone());
                Ok(auth_response)
            }
            Err(e) => {
                error!("Authentication failed for user {}: {:?}", username, e);
                // Assuming authenticate returns a displayable error
                Err(JellyfinError::Authentication(format!("Authentication failed: {}", e)))
            }
        }
    }

    /// Initialize the session manager, report capabilities, and establish WebSocket connection.
    #[instrument(skip(self, shutdown_tx), fields(device_id))] // Update skip parameter name
    pub async fn initialize_session(&mut self, device_id: &str, shutdown_tx: broadcast::Sender<()>) -> Result<(), JellyfinError> { // Change parameter type
        info!("Initializing session with DeviceId: {}", device_id);

        let (api_key, user_id) = self.ensure_authenticated()?;
        let api_key = api_key.to_string(); // Clone needed parts
        let user_id = user_id.to_string();

        debug!("Creating session manager for user_id: {}, play_session_id: {}", user_id, self.play_session_id);
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
        debug!("Session manager created and stored.");

        // Initialize WebSocket
        self._initialize_websocket(&api_key, device_id, shutdown_tx).await?; // Pass the sender

        info!("Session initialization complete.");
        Ok(())
    }

    /// Get root items from the user's library (Views)
    #[instrument(skip(self))]
    pub async fn get_items(&self) -> Result<Vec<MediaItem>, JellyfinError> {
        debug!("Fetching root library items (Views)");
        let (_, user_id) = self.ensure_authenticated()?;
        let path = format!("/Users/{}/Views", user_id);
        let response: ItemsResponse = self._get_json(&path, None).await?;
        debug!("Successfully fetched {} root items", response.items.len());
        Ok(response.items)
    }

    /// Get child items of a folder/collection
    #[instrument(skip(self), fields(parent_id))]
    pub async fn get_items_by_parent_id(&self, parent_id: &str) -> Result<Vec<MediaItem>, JellyfinError> {
        debug!("Fetching items with parent_id: {}", parent_id);
        let (_, user_id) = self.ensure_authenticated()?;
        let path = format!("/Users/{}/Items", user_id);
        let params = [("ParentId", parent_id)];
        let response: ItemsResponse = self._get_json(&path, Some(&params)).await?;
        debug!("Successfully fetched {} items for parent {}", response.items.len(), parent_id);
        Ok(response.items)
    }

    /// Get full details for multiple items by their IDs
    #[instrument(skip(self), fields(item_count = item_ids.len()))]
    pub async fn get_items_details(&self, item_ids: &[String]) -> Result<Vec<MediaItem>, JellyfinError> {
        debug!("Fetching details for {} item(s)", item_ids.len());
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
        debug!("Successfully fetched details for {} items", response.items.len());
        Ok(response.items)
    }

    /// Get streaming URL for an item
    pub fn get_stream_url(&self, item_id: &str) -> Result<String, JellyfinError> {
        debug!("Generating stream URL for item_id: {}", item_id);
        let (api_key, _) = self.ensure_authenticated()?;
        // Note: Using Audio path, adjust if video/other types are needed
        let url = format!("{}/Audio/{}/stream?static=true&api_key={}", self.server_url, item_id, api_key);
        debug!("Generated stream URL: {}", url);
        Ok(url)
    }

    // Removed incorrect _report_playback helper that delegated to SessionManager

    /// Report playback started to Jellyfin server via HTTP POST.
    #[instrument(skip(self, report), fields(item_id = %report.base.item_id))]
    pub async fn report_playback_start(&self, report: &PlaybackStartReport) -> Result<(), JellyfinError> {
        info!("Reporting playback start for item_id: {}", report.base.item_id);
        self._post_json_no_content("/Sessions/Playing", report).await
    }

    /// Report playback progress to Jellyfin server via HTTP POST.
    #[instrument(skip(self, report), fields(item_id = %report.base.item_id, position_ticks = report.base.position_ticks))]
    pub async fn report_playback_progress(&self, report: &PlaybackProgressReport) -> Result<(), JellyfinError> {
        // Avoid overly verbose logging for progress updates
        trace!("Reporting playback progress for item_id: {}, PositionTicks: {}", report.base.item_id, report.base.position_ticks);
        self._post_json_no_content("/Sessions/Playing/Progress", report).await
    }

    /// Report playback stopped to Jellyfin server via HTTP POST.
    #[instrument(skip(self, report), fields(item_id = %report.base.item_id))]
    pub async fn report_playback_stopped(&self, report: &PlaybackStopReport) -> Result<(), JellyfinError> {
        info!("Reporting playback stopped for item_id: {}", report.base.item_id);
        self._post_json_no_content("/Sessions/Playing/Stopped", report).await
    }

    // Removed set_player method as WebSocketHandler no longer holds the player directly.
    // Channels are passed during listener startup.

    /// Spawns the WebSocket listener task.
    /// Requires `initialize_session` to have been called successfully.
    #[instrument(skip(self, player_command_tx, player_state_rx, shutdown_rx))]
    pub async fn start_websocket_listener(
        &mut self,
        player_command_tx: mpsc::Sender<PlayerCommand>, // Sender for commands TO player
        player_state_rx: broadcast::Receiver<InternalPlayerStateUpdate>, // Receiver for state FROM player
        shutdown_rx: broadcast::Receiver<()>, // Receiver for app shutdown
    ) -> Result<(), JellyfinError> {
        info!("Attempting to start WebSocket listener task...");

        let ws_handler_arc = match self.websocket_handler.clone() { // Clone Arc to move into task
            Some(arc) => arc,
            None => {
                error!("Cannot start listener: WebSocket handler not initialized.");
                return Err(JellyfinError::WebSocketError("WebSocket handler not initialized".to_string()));
            }
        };

        // Player instance check removed, as the handler doesn't hold it directly anymore.
        // The necessary channels are passed as arguments.


        // No longer need to clone the AtomicBool
        debug!("Spawning WebSocket listener task...");

        // Clone the command sender for the incoming message handler within the listener task
        let command_tx_clone = player_command_tx.clone();

        let handle = tokio::spawn(async move {
            trace!("WebSocket listener task started execution.");
            // Lock the handler Arc within the task to prepare and listen
            let prepared_result = {
                 let mut handler_guard = ws_handler_arc.lock().await;
                 // Pass the necessary channels to prepare_for_listening
                 handler_guard.prepare_for_listening(command_tx_clone, player_state_rx, shutdown_rx)
            }; // MutexGuard dropped here

            if let Some(mut prepared_handler) = prepared_result {
                debug!("Prepared handler obtained, starting listen_for_commands loop...");
                if let Err(e) = prepared_handler.listen_for_commands().await {
                    error!("WebSocket listener loop exited with error: {:?}", e);
                } else {
                    debug!("WebSocket listener loop exited gracefully.");
                }
            } else {
                error!("Failed to obtain prepared handler for WebSocket listener (was it connected?).");
            }
            debug!("WebSocket listener task finished execution.");
        });

        // Store the handle
        let mut handle_guard = self.websocket_listener_handle.lock().await;
        *handle_guard = Some(handle);
        info!("WebSocket listener task spawned and handle stored.");
        Ok(())
    }

    /// Take the JoinHandle for the WebSocket listener task, if it exists.
    /// This allows the caller to await the task's completion during shutdown.
    #[instrument(skip(self))]
    pub async fn take_websocket_handle(&mut self) -> Option<JoinHandle<()>> {
        debug!("Attempting to take WebSocket listener task handle");
        let mut handle_guard = self.websocket_listener_handle.lock().await;
        let handle = handle_guard.take();
        if handle.is_some() {
            debug!("WebSocket listener task handle taken");
        } else {
            debug!("No WebSocket listener task handle was present");
        }
        handle
    }


    // Removed duplicate play_session_id function definition. The original is at line 128.
    // --- Getter methods (primarily for testing/debugging) ---
    pub fn get_server_url(&self) -> &str { &self.server_url }
    pub fn get_api_key(&self) -> Option<&str> { self.api_key.as_deref() }
    pub fn get_user_id(&self) -> Option<&str> { self.user_id.as_deref() }
}

// --- Trait Implementation for Real Client ---

#[async_trait::async_trait]
impl JellyfinApiContract for JellyfinClient {
    async fn get_items_details(&self, item_ids: &[String]) -> Result<Vec<MediaItem>, JellyfinError> {
        // Delegate to existing method
        JellyfinClient::get_items_details(self, item_ids).await
    }

    async fn get_audio_stream_url(&self, item_id: &str) -> Result<String, JellyfinError> {
        // Delegate to existing method (Note: original get_stream_url wasn't async, adjust if needed or keep sync here)
        // Assuming the intent was async based on mock usage, let's make the trait async.
        // The original get_stream_url needs to be made async or called differently.
        // For now, let's call the sync version and wrap it.
        // TODO: Review if get_stream_url should be async in JellyfinClient.
        JellyfinClient::get_stream_url(self, item_id) // Original is sync
    }

    async fn report_playback_start(&self, report: &PlaybackStartReport) -> Result<(), JellyfinError> {
        JellyfinClient::report_playback_start(self, report).await
    }

    async fn report_playback_stopped(&self, report: &PlaybackStopReport) -> Result<(), JellyfinError> {
        JellyfinClient::report_playback_stopped(self, report).await
    }

    async fn report_playback_progress(&self, report: &PlaybackProgressReport) -> Result<(), JellyfinError> {
        JellyfinClient::report_playback_progress(self, report).await
    }

    fn play_session_id(&self) -> &str {
        JellyfinClient::play_session_id(self)
    }
}

