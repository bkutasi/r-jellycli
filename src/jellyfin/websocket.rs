use futures::StreamExt;
use futures_util::SinkExt;
use serde::Deserialize;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tokio::time::Duration;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};
use tokio_tungstenite::tungstenite::Message;
use url::Url;
use log::{debug, error, info, warn};

use crate::jellyfin::api::JellyfinClient;
use crate::jellyfin::models::MediaItem;
use crate::player::Player;

// Define WebSocket message types
#[derive(Debug, Deserialize, Clone)]
pub struct WebSocketMessage {
    #[serde(rename = "MessageType")]
    pub message_type: String,
    #[serde(rename = "Data")]
    pub data: Option<serde_json::Value>,
}

// General command message format
#[derive(Debug, Deserialize, Clone)]
pub struct GeneralCommand {
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "Arguments")]
    pub arguments: Option<serde_json::Value>,
}

// PlayState command format
#[derive(Debug, Deserialize, Clone)]
pub struct PlayStateCommand {
    #[serde(rename = "Command")]
    pub command: String,
}

// Play command format
#[derive(Debug, Deserialize, Clone)]
pub struct PlayCommand {
    #[serde(rename = "ItemIds")]
    pub item_ids: Vec<String>,
    #[serde(rename = "StartIndex")]
    pub start_index: Option<i32>,
    #[serde(rename = "PlayCommand")]
    pub play_command: String,
}

pub struct WebSocketHandler {
    server_url: String,
    api_key: String,
    device_id: String,
    session_id: Option<String>,
    jellyfin_client: JellyfinClient,
    player: Option<Arc<Mutex<Player>>>,
    ws_stream: Option<WebSocketStream<MaybeTlsStream<TcpStream>>>,
}

impl WebSocketHandler {
    pub fn new(jellyfin_client: JellyfinClient, server_url: &str, api_key: &str, device_id: &str) -> Self {
        WebSocketHandler {
            server_url: server_url.to_string(),
            api_key: api_key.to_string(),
            device_id: device_id.to_string(),
            session_id: None,
            jellyfin_client,
            player: None,
            ws_stream: None,
        }
    }
    
    pub fn with_session_id(mut self, session_id: &str) -> Self {
        self.session_id = Some(session_id.to_string());
        self
    }

    pub fn set_player(&mut self, player: Arc<Mutex<Player>>) {
        self.player = Some(player);
    }

    pub async fn connect(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        // Parse server URL to determine if we should use wss or ws
        let parsed_url = Url::parse(&self.server_url)?;
        let scheme = if parsed_url.scheme() == "https" { "wss" } else { "ws" };

        // Construct WebSocket URL with api_key and deviceId
        let host = parsed_url.host_str().unwrap_or("localhost");
        let port = parsed_url.port().unwrap_or(if scheme == "wss" { 443 } else { 80 });
        let path = parsed_url.path();

        // Build the WebSocket URL - IMPORTANT: Exactly like jellycli
        // Do NOT include sessionId parameter for initial connection
        // Format: {scheme}://{host}:{port}{path}socket?api_key={api_key}&deviceId={device_id}
        let ws_url = format!(
            "{}://{}:{}{}socket?api_key={}&deviceId={}",
            scheme, host, port, path, self.api_key, self.device_id
        );

        debug!("Connecting to WebSocket using jellycli-compatible format: {}", ws_url);

        let url = Url::parse(&ws_url)?;
        let (ws_stream, _) = connect_async(url).await?;

        info!("WebSocket connected");
        debug!("WebSocket connection established successfully.");
        // Store the stream temporarily to send the initial message
        let mut temp_ws_stream = ws_stream;

        // Send initial KeepAlive message immediately after connecting
        // Jellyfin expects this to keep the connection registered properly
        let keep_alive_msg = r#"{"MessageType": "KeepAlive"}"#;
        debug!("Sending initial KeepAlive message: {}", keep_alive_msg);
        if let Err(e) = temp_ws_stream.send(Message::Text(keep_alive_msg.to_string())).await {
            error!("Failed to send initial KeepAlive message: {}", e);
            // Return error if the initial message fails, as the connection is likely unusable
            return Err(Box::new(e));
        }
        debug!("Initial KeepAlive message sent successfully.");
        self.ws_stream = Some(temp_ws_stream); // Store the stream after sending the message

        Ok(())
    }

    pub fn prepare_for_listening(&mut self, shutdown_signal: Arc<AtomicBool>) -> Option<PreparedWebSocketHandler> {
        if self.ws_stream.is_none() {
            error!("Cannot prepare for listening: WebSocket not connected");
            return None;
        }

        let ws_stream = self.ws_stream.take().unwrap();
        let player = self.player.clone();
        let jellyfin_client = self.jellyfin_client.clone();

        Some(PreparedWebSocketHandler {
            websocket: ws_stream,
            player,
            shutdown_signal,
            jellyfin_client,
        })
    }
}

/// PreparedWebSocketHandler is used to handle WebSocket operations without holding a mutex
pub struct PreparedWebSocketHandler {
    websocket: WebSocketStream<MaybeTlsStream<TcpStream>>,
    player: Option<Arc<Mutex<Player>>>,
    shutdown_signal: Arc<AtomicBool>,
    jellyfin_client: JellyfinClient,
}

impl PreparedWebSocketHandler {
    /// Listen for commands from the Jellyfin server
    pub async fn listen_for_commands(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        log::trace!("[WS Listen] Entered listen_for_commands");
        debug!("[WS Listen] Entering listen_for_commands function.");
        let player = self.player.clone();
        debug!("[WS Listen] Player cloned: {}", if player.is_some() { "Some" } else { "None" });
        let mut ping_interval = tokio::time::interval(Duration::from_secs(30));
        
        debug!("[WS Listen] Ping interval created.");
        debug!("Starting WebSocket listening loop with ping every 30s");
        
        // Process incoming messages
        debug!("[WS Listen] Entering main loop..."); // Added log
        loop {
            // Check for shutdown signal before processing any messages
            let shutdown_state = self.shutdown_signal.load(Ordering::SeqCst); // Read the value
            debug!("[WS Listen] Checking shutdown signal at loop start. Value: {}", !shutdown_state); // Log the INVERTED value for clarity in this message
            if !shutdown_state { // Check 1 - Corrected: Exit if signal is FALSE
                debug!("[WS Listen] Shutdown signal is FALSE at loop start. Exiting."); // Corrected log
                debug!("WebSocket listener received shutdown signal, sending Close frame and exiting...");
                // Attempt to send a close frame gracefully
                if let Err(e) = self.websocket.send(Message::Close(None)).await {
                    warn!("Failed to send WebSocket close frame during shutdown: {}", e);
                }
                // Even if sending close fails, break the loop to terminate the task
                break;
            }
            
            debug!("[WS Listen] Entering select! statement..."); // Added log
            tokio::select! {
                // Branch 1: Message received or stream ended
                maybe_message = self.websocket.next() => { // Renamed to maybe_message for clarity
                    debug!("[WS Listen] select! resolved: websocket.next() completed."); // Added log
                    match maybe_message {
                        Some(Ok(msg)) => { // Message received successfully
                            // Existing logic for handling Ok(msg)
                            if let Message::Text(text) = msg {
                                debug!("[WS] Received Text message.");
                                debug!("Received WebSocket message: {}", text);

                                // Parse the message
                                if let Ok(ws_msg) = serde_json::from_str::<WebSocketMessage>(&text) {
                                    self.handle_message(ws_msg, player.clone()).await;
                                } else {
                                    warn!("Failed to parse WebSocket message: {}", text);
                                }
                            } else if let Message::Ping(data) = msg {
                                debug!("[WS] Received Ping, sending Pong...");
                                if let Err(e) = self.websocket.send(Message::Pong(data)).await {
                                    error!("Failed to send pong: {}", e);
                                } else { // Corrected log placement
                                    debug!("[WS] Pong sent successfully.");
                                }
                            } else if msg.is_close() {
                                debug!("[WS Listen] Received Close frame from server. Exiting loop."); // Added log
                                debug!("Received close frame");
                                debug!("[WS] Received Close frame.");
                                break; // Exit loop on Close frame
                            } else {
                                debug!("[WS] Received other message type: {:?}", msg); // Log other types
                            }
                        },
                        Some(Err(e)) => { // Error reading from WebSocket
                            debug!("[WS Listen] WebSocket read error occurred: {}. Returning Err.", e); // Added log
                            error!("WebSocket read error: {}. Attempting graceful close.", e);
                            let _ = self.websocket.close(None).await;
                            return Err(format!("WebSocket read error: {}", e).into()); // Exit function with error
                        },
                        None => { // Stream ended gracefully (server closed connection without Close frame?)
                            debug!("[WS Listen] WebSocket stream ended (returned None). Exiting loop."); // Added log
                            break; // Exit loop if stream ends
                        }
                    }
                },
                // Branch 2: Ping interval ticked
                _ = ping_interval.tick() => {
                    debug!("[WS Listen] select! resolved: ping_interval.tick() completed."); // Added log
                    // Check for shutdown signal before sending ping
                    // Corrected check: Exit if signal is FALSE
                    if !self.shutdown_signal.load(Ordering::SeqCst) { // Check 2 - Corrected
                        debug!("[WS Listen] Shutdown signal is FALSE before sending ping. Exiting loop."); // Corrected log
                        break; // Exit loop
                    }

                    debug!("Sending ping to keep WebSocket alive");
                    debug!("[WS] Sending periodic Ping...");
                    if let Err(e) = self.websocket.send(Message::Ping(vec![])).await {
                        error!("Failed to send ping: {}. Attempting graceful close.", e);
                        debug!("[WS Listen] Failed to send ping: {}. Returning Err.", e); // Added log
                        let _ = self.websocket.close(None).await;
                        return Err(format!("Failed to send ping: {}", e).into()); // Exit function with error
                    } else { // Added else block for successful ping log
                         debug!("[WS] Ping sent successfully.");
                    }
                }
            }
            debug!("[WS Listen] Reached end of loop iteration."); // Added log
        }

        debug!("WebSocket listener exiting...");
        debug!("[WS] Exiting listening loop.");
        Ok(())
    }

    async fn handle_message(&self, message: WebSocketMessage, player: Option<Arc<Mutex<Player>>>) {
        if player.is_none() {
            warn!("Cannot handle remote control command: player not set");
            return;
        }

        let player = player.unwrap();

        match message.message_type.as_str() {
            "ForceKeepAlive" => {
                // Just a keep-alive, nothing to do
                debug!("Received keep-alive message");
            },
            "GeneralCommand" => {
                if let Some(data) = message.data {
                    if let Ok(cmd) = serde_json::from_value::<GeneralCommand>(data) {
                        self.handle_general_command(cmd, player.clone()).await;
                    }
                }
            },
            "PlayState" => {
                if let Some(data) = message.data {
                    if let Ok(cmd) = serde_json::from_value::<PlayStateCommand>(data) {
                        self.handle_playstate_command(cmd, player.clone()).await;
                    }
                }
            },
            "Play" => {
                if let Some(data) = message.data {
                    if let Ok(cmd) = serde_json::from_value::<PlayCommand>(data) {
                        self.handle_play_command(cmd, player.clone()).await;
                    }
                }
            },
            _ => {
                warn!("Unhandled message type: {}", message.message_type);
            }
        }
    }

    async fn handle_general_command(&self, command: GeneralCommand, player: Arc<Mutex<Player>>) {
        debug!("Handling general command: {}", command.name);

        match command.name.as_str() {
            "SetVolume" => {
                // Set volume
                if let Some(arguments) = &command.arguments {
                    if let Some(vol) = arguments.get("Volume").and_then(|v| v.as_u64()) {
                        debug!("Received SetVolume command: {}", vol);
                        let mut player_guard = player.lock().await;
                        if let Ok(vol) = u8::try_from(vol) {
                            if vol <= 100 {
                                // Set volume (implement in player)
                                player_guard.set_volume(vol).await;
                            }
                        }
                    }
                }
            },
            "ToggleMute" => {
                // Toggle mute (implement in player)
                debug!("Toggling mute");
                let mut player_guard = player.lock().await;
                player_guard.toggle_mute().await;
            },
            _ => {
                warn!("Unhandled general command: {}", command.name);
            }
        }
    }

    async fn handle_playstate_command(&self, command: PlayStateCommand, player: Arc<Mutex<Player>>) {
        debug!("Handling playstate command: {}", command.command);

        match command.command.as_str() {
            "PlayPause" => {
                // Toggle play/pause
                debug!("Toggle play/pause");
                let mut player_guard = player.lock().await;
                player_guard.play_pause().await;
            },
            "NextTrack" => {
                // Skip to next track
                debug!("Skip to next track");
                let mut player_guard = player.lock().await;
                player_guard.next().await;
            },
            "PreviousTrack" => {
                // Skip to previous track
                debug!("Skip to previous track");
                let mut player_guard = player.lock().await;
                player_guard.previous().await;
            },
            "Pause" => {
                // Pause playback
                debug!("Pause playback");
                let mut player_guard = player.lock().await;
                player_guard.pause().await;
            },
            "Unpause" => {
                // Resume playback
                debug!("Resume playback");
                let mut player_guard = player.lock().await;
                player_guard.resume().await;
            },
            "Stop" | "StopMedia" => {
                // Stop playback
                debug!("Stop playback");
                let mut player_guard = player.lock().await;
                player_guard.stop().await;
            },
            _ => {
                warn!("Unhandled playstate command: {}", command.command);
            }
        }
    }

    async fn handle_play_command(&self, command: PlayCommand, player: Arc<Mutex<Player>>) {
        debug!("Handling play command: {} with {} items", command.play_command, command.item_ids.len());

        let item_ids = command.item_ids;
        let start_index = command.start_index.unwrap_or(0) as usize;

        if item_ids.is_empty() {
            warn!("Received play command with empty item list");
            return;
        }

        if start_index >= item_ids.len() {
            warn!("Start index {} is out of bounds (item count: {})", start_index, item_ids.len());
            return;
        }

        // Fetch item details using the JellyfinClient
        info!("Fetching details for {} items to play...", item_ids.len());
        match self.jellyfin_client.get_items_details(&item_ids).await {
            Ok(mut media_items) => {
                info!("Successfully fetched details for {} items", media_items.len());

                // Sort items based on the original order in item_ids (API might not preserve order)
                media_items.sort_by_key(|item| item_ids.iter().position(|id| id == &item.id).unwrap_or(usize::MAX));

                // Prepare the items to play starting from start_index
                let items_to_play: Vec<MediaItem> = media_items.into_iter().skip(start_index).collect();

                if items_to_play.is_empty() {
                    warn!("No items left to play after applying start_index {}", start_index);
                    return;
                }

                debug!("Items to play ({}): {:?}", items_to_play.len(), items_to_play.iter().map(|i| i.id.clone()).collect::<Vec<_>>());

                // Lock the player and update the queue
                let mut player_guard = player.lock().await;

                // Determine action based on PlayCommand type
                match command.play_command.as_str() {
                    "PlayNow" => {
                        debug!("Clearing queue and playing items now");
                        player_guard.clear_queue().await; // Assumes Player has clear_queue()
                        player_guard.add_items(items_to_play); // Assumes Player has add_items(Vec<MediaItem>)
                        player_guard.play_from_start().await; // Assumes Player has play_from_start()
                    },
                    "PlayNext" => {
                        debug!("Adding items to the start of the queue");
                        // Requires a Player method like `add_items_next` or similar
                        // player_guard.add_items_next(items_to_play);
                        warn!("PlayNext functionality not yet implemented in Player"); // Placeholder
                    },
                    "PlayLast" => {
                        debug!("Adding items to the end of the queue");
                        player_guard.add_items(items_to_play); // Assumes add_items appends
                    },
                    _ => {
                        warn!("Unhandled PlayCommand type: {}", command.play_command);
                    }
                }
            },
            Err(e) => {
                error!("Failed to fetch item details for play command: {:?}", e);
                // Optionally, send a feedback message over WebSocket if possible/needed
            }
        }
    }
}

