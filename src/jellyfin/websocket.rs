//! Handles the WebSocket connection to the Jellyfin server for real-time communication.
//! Manages sending player state updates and receiving remote control commands.

// Removed unused imports: PlaybackStartReport, PlaybackStopReport
use tracing::instrument;
use crate::jellyfin::models_playback::{GeneralCommand, PlayCommand, PlayStateCommand}; // Import directly
use futures::{SinkExt, StreamExt};
use tracing::{debug, error, info, trace, warn}; // Replaced log with tracing
use serde::{Deserialize, Serialize};
// Removed unused imports: AtomicBool, Ordering
use std::sync::Arc;
use std::time::Duration;
use tokio::{net::TcpStream, sync::broadcast}; // Add broadcast import
use tokio::sync::{mpsc, Mutex};
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};
use tokio_tungstenite::tungstenite::Message;
use url::Url;

use crate::jellyfin::api::JellyfinClient;
use crate::jellyfin::models::MediaItem;
// Removed unused import: use crate::jellyfin::models_playback::*;
use crate::player::Player;

use super::ws_incoming_handler; // Import the new handler module

// --- WebSocket Message Structs (Incoming & Outgoing) ---

/// Represents a generic incoming message structure from the WebSocket.
#[derive(Debug, Deserialize, Clone)]
pub struct WebSocketMessage {
    #[serde(rename = "MessageType")]
    pub message_type: String,
    #[serde(rename = "Data")]
    pub data: Option<serde_json::Value>,
}

/// Represents a generic outgoing message structure for the WebSocket.
#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct OutgoingWsMessage<T> {
    #[serde(rename = "MessageType")]
    pub message_type: String,
    #[serde(rename = "Data")]
    pub data: T,
}

// --- Player State Update Channel ---

/// Enum representing different states or events reported by the Player.
/// These are sent over an MPSC channel to the WebSocket handler.
#[derive(Debug, Clone)]
pub enum PlayerStateUpdate {
    /// Playback started for a specific item.
    Started { item: MediaItem },
    /// Playback stopped for a specific item.
    Stopped {
        item_id: String,
        final_position_ticks: i64,
    },
    /// Playback progress update. Includes essential state for reporting.
    Progress {
        item_id: String,
        position_ticks: i64,
        is_paused: bool,
        volume: i32,
        is_muted: bool,
    },
    /// Volume or mute status changed.
    VolumeChanged {
        volume: i32,
        is_muted: bool,
    },
    // TODO: Add other states if needed (e.g., QueueChanged, Error)
}

// --- WebSocket Handler Setup ---

/// Manages the initial setup and connection of the WebSocket.
pub struct WebSocketHandler {
    server_url: String,
    api_key: String,
    device_id: String,
    jellyfin_client: JellyfinClient,
    player: Option<Arc<Mutex<Player>>>,
    ws_stream: Option<WebSocketStream<MaybeTlsStream<TcpStream>>>,
    player_update_tx: Option<mpsc::UnboundedSender<PlayerStateUpdate>>,
    // session_id is not used for the initial connection based on jellycli behavior
    // session_id: Option<String>,
}

impl WebSocketHandler {
    /// Creates a new WebSocketHandler instance.
    pub fn new(
        jellyfin_client: JellyfinClient,
        server_url: &str,
        api_key: &str,
        device_id: &str,
    ) -> Self {
        WebSocketHandler {
            server_url: server_url.trim_end_matches('/').to_string(), // Ensure no trailing slash
            api_key: api_key.to_string(),
            device_id: device_id.to_string(),
            jellyfin_client,
            player: None,
            ws_stream: None,
            player_update_tx: None,
            // session_id: None,
        }
    }

    // pub fn with_session_id(mut self, session_id: &str) -> Self {
    //     self.session_id = Some(session_id.to_string());
    //     self
    // }

    /// Sets the Player instance for the handler.
    /// If the update channel sender exists, it attempts to update the player instance.
    pub fn set_player(&mut self, player: Arc<Mutex<Player>>) {
        self.player = Some(player.clone());
        debug!("[WS Handler] Player instance set.");
        // If the channel sender already exists, update the player instance
        if let Some(tx) = &self.player_update_tx {
            let tx_clone = tx.clone();
            // Spawn a task to avoid blocking while holding the handler's lock
            tokio::spawn(async move {
                let mut player_guard = player.lock().await;
                // Assume Player has a method like `set_update_sender`
                player_guard.set_update_sender(tx_clone);
                debug!("[WS Handler] Updated player instance with state update sender.");
            });
        } else {
            debug!("[WS Handler] Player set, but update sender not yet available (will be set during prepare).");
        }
    }

    /// Checks if the Player instance has been set.
    pub fn is_player_set(&self) -> bool {
        self.player.is_some()
    }

    /// Sets the sender part of the MPSC channel for player state updates.
    /// This is typically called internally during `prepare_for_listening`.
    fn set_player_update_tx(&mut self, tx: mpsc::UnboundedSender<PlayerStateUpdate>) {
        self.player_update_tx = Some(tx);
        debug!("[WS Handler] Player update channel sender set.");
    }

    /// Establishes the WebSocket connection to the Jellyfin server.
    #[instrument(skip(self), fields(device_id = %self.device_id))]
    pub async fn connect(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let parsed_url = Url::parse(&self.server_url)?;
        let scheme = if parsed_url.scheme() == "https" { "wss" } else { "ws" };
        let host = parsed_url.host_str().ok_or("Server URL missing host")?;
        let port_str = parsed_url.port().map_or_else(
            || if scheme == "wss" { ":443".to_string() } else { ":80".to_string() }, // Default ports if not specified
            |p| format!(":{}", p),
        );
        // Use default port only if not explicitly specified in the URL
        let host_port = format!("{}{}", host, if parsed_url.port().is_some() { port_str } else { "".to_string() });


        let path = parsed_url.path(); // Includes leading '/' if present

        // Construct WebSocket URL exactly like jellycli:
        // {scheme}://{host}:{port}{path}/socket?api_key={api_key}&deviceId={device_id}
        // Note: No sessionId parameter here.
        let ws_url_str = format!(
            "{}://{}{}/socket?api_key={}&deviceId={}", // Ensure path starts with / if needed
            scheme,
            host_port,
            if path == "/" { "" } else { path }, // Avoid double slash if path is just "/"
            self.api_key,
            self.device_id
        );

        info!("Attempting WebSocket connection to: {}...", host_port); // Log host only for brevity
        debug!("[WS Connect] Full WebSocket URL: {}", ws_url_str);

        let ws_url = Url::parse(&ws_url_str)?;
        let (ws_stream, response) = connect_async(ws_url).await?;

        debug!("[WS Connect] WebSocket connection established. Response status: {}", response.status());
        info!("WebSocket connected successfully to {}", host_port);

        // Store the stream temporarily to send the initial message
        let mut temp_ws_stream = ws_stream;

        // Send initial KeepAlive immediately after connecting (required by Jellyfin)
        let keep_alive_msg = r#"{"MessageType": "KeepAlive"}"#;
        debug!("[WS Connect] Sending initial KeepAlive message.");
        trace!("[WS Connect] KeepAlive payload: {}", keep_alive_msg);
        if let Err(e) = temp_ws_stream.send(Message::Text(keep_alive_msg.to_string())).await {
            error!("[WS Connect] Failed to send initial KeepAlive message: {}", e);
            // Return error if the initial message fails, as the connection is likely unusable
            return Err(format!("Failed to send initial KeepAlive: {}", e).into());
        }
        debug!("[WS Connect] Initial KeepAlive message sent successfully.");

        self.ws_stream = Some(temp_ws_stream); // Store the stream

        Ok(())
    }


    /// Prepares the handler for the listening loop by creating the MPSC channel
    /// and returning a `PreparedWebSocketHandler` which owns the stream and channel receiver.
    pub fn prepare_for_listening(
        &mut self,
        // Change signature to accept broadcast::Receiver
        shutdown_rx: broadcast::Receiver<()>,
    ) -> Option<PreparedWebSocketHandler> {
        let ws_stream = match self.ws_stream.take() {
            Some(stream) => stream,
            None => {
                error!("[WS Prepare] Cannot prepare for listening: WebSocket not connected or already taken.");
                return None;
            }
        };

        // Create the MPSC channel for player state updates
        let (tx, rx) = mpsc::unbounded_channel::<PlayerStateUpdate>();
        self.set_player_update_tx(tx.clone()); // Store the sender

        // If player instance already exists, update its sender immediately
        if let Some(player_arc) = &self.player {
            let player_clone: Arc<Mutex<Player>> = Arc::clone(player_arc); // <<< Added type annotation here
            // Spawn task to update the player instance asynchronously
            tokio::spawn(async move {
                let mut player_guard = player_clone.lock().await;
                player_guard.set_update_sender(tx); // Pass the sender directly
                debug!("[WS Prepare] Updated existing player instance with sender.");
            });
        } else {
            debug!("[WS Prepare] Player instance not set yet; sender stored for later use.");
        }

        debug!("[WS Prepare] Prepared for listening. Handing over stream and receiver.");
        Some(PreparedWebSocketHandler {
            websocket: ws_stream,
            player: self.player.clone(), // Clone Arc<Mutex<Player>>
            shutdown_rx, // Store the receiver
            jellyfin_client: self.jellyfin_client.clone(),
            player_update_rx: rx, // Pass the receiver
        })

    }
} // End of impl WebSocketHandler

// --- Active WebSocket Listener ---

/// Handles the active WebSocket listening loop, processing incoming messages
/// and sending outgoing state updates. Owns the WebSocket stream and MPSC receiver.
pub struct PreparedWebSocketHandler {
    websocket: WebSocketStream<MaybeTlsStream<TcpStream>>,
    player: Option<Arc<Mutex<Player>>>, // Keep Arc<Mutex<Player>> for state access
    // Replace AtomicBool with broadcast::Receiver
    shutdown_rx: broadcast::Receiver<()>,
    jellyfin_client: JellyfinClient,
    player_update_rx: mpsc::UnboundedReceiver<PlayerStateUpdate>,
}

impl PreparedWebSocketHandler {
    /// Listens for commands from the Jellyfin server and player updates.
    /// This is the main loop for the active WebSocket connection.
    // Make self mutable for shutdown_rx.recv()
    #[instrument(skip_all, name = "ws_listener_loop")]
    pub async fn listen_for_commands(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        info!("Starting WebSocket listening loop...");
        let mut ping_interval = tokio::time::interval(Duration::from_secs(30)); // Standard interval

        loop {
            // No longer need manual check here, handled by select!

            trace!("[WS Listen] Entering select! statement.");
            tokio::select! {
                biased; // Prioritize checking incoming messages slightly

                // --- Branch 1: Message from Jellyfin Server ---
                maybe_message = self.websocket.next() => {
                    trace!("[WS Listen] select! resolved: websocket.next()");
                    match maybe_message {
                        Some(Ok(msg)) => {
                            if !self.process_incoming_message(msg).await? {
                                // process_incoming_message returns false if Close frame received
                                info!("Received Close frame from server. Exiting loop.");
                                break;
                            }
                        },
                        Some(Err(e)) => {
                            error!("[WS Listen] WebSocket read error: {}. Closing connection.", e);
                            let _ = self.websocket.close(None).await; // Attempt graceful close
                            return Err(format!("WebSocket read error: {}", e).into()); // Exit function with error
                        },
                        None => {
                            info!("WebSocket stream ended (returned None). Exiting loop.");
                            break; // Exit loop if stream ends
                        }
                    }
                },

                // --- Branch 2: Player State Update from Player ---
                maybe_update = self.player_update_rx.recv() => {
                    trace!("[WS Listen] select! resolved: player_update_rx.recv()");
                    if let Some(update) = maybe_update {
                        if let Err(e) = self.process_player_update(update).await {
                             error!("[WS Listen] Failed to process player update and send WS message: {}", e);
                             // Decide if this error is fatal. Continuing for now.
                        }
                    } else {
                        info!("Player update channel closed. Exiting loop.");
                        break; // Exit if the sender side is dropped
                    }
                },

                // --- Branch 3: Ping Interval ---
                _ = ping_interval.tick() => {
                    trace!("[WS Listen] select! resolved: ping_interval.tick()"); // No need to check shutdown here anymore
                    if let Err(e) = self.send_keep_alive_ping().await {
                         error!("[WS Listen] Failed to send WebSocket ping: {}. Returning error.", e);
                         // Return the error *before* awaiting the close, as 'e' might not be Send
                         let error_to_return = e; // Move error out
                         // Attempt graceful close without awaiting if returning immediately
                         // Or, if close MUST be awaited, map 'e' to a Send error first.
                         // For simplicity, let's return first. The caller can handle cleanup.
                         return Err(error_to_return);
                         // let _ = self.websocket.close(None).await; // This await caused the Send issue
                    }
                },

                // --- Branch 4: Shutdown Signal ---
                _ = self.shutdown_rx.recv() => {
                    trace!("[WS Listen] select! resolved: shutdown_rx.recv()");
                    info!("Shutdown signal received via broadcast channel. Exiting loop.");
                    break; // Exit the loop
                }
            }
            trace!("[WS Listen] Reached end of loop iteration.");
        }

        info!("Listener loop exited.");
        self.close_websocket().await; // Attempt graceful close after loop exit
        Ok(())
    }

    /// Processes a single incoming message from the WebSocket stream.
    /// Returns `Ok(true)` to continue listening, `Ok(false)` on Close frame, `Err` on error.
    async fn process_incoming_message(&mut self, msg: Message) -> Result<bool, Box<dyn std::error::Error>> {
        match msg {
            Message::Text(text) => {
                debug!("[WS Recv] Received Text message.");
                trace!("[WS Recv] Payload: {}", text);
                match serde_json::from_str::<WebSocketMessage>(&text) {
                    Ok(ws_msg) => self.handle_parsed_message(ws_msg).await,
                    Err(e) => {
                        warn!("[WS Recv] Failed to parse WebSocket message: {}. Content: '{}'", e, text);
                    }
                }
            }
            Message::Binary(bin) => {
                debug!("[WS Recv] Received Binary message ({} bytes). Ignoring.", bin.len());
                // Handle binary data if needed, otherwise ignore.
            }
            Message::Ping(data) => {
                debug!("[WS Recv] Received Ping from server.");
                trace!("[WS Recv] Ping payload: {:?}", data);
                // Respond with Pong
                if let Err(e) = self.websocket.send(Message::Pong(data)).await {
                    error!("[WS Send] Failed to send Pong response: {}", e);
                    // Consider if this error is fatal
                } else {
                    debug!("[WS Send] Pong response sent successfully.");
                }
            }
            Message::Pong(data) => {
                debug!("[WS Recv] Received Pong from server.");
                trace!("[WS Recv] Pong payload: {:?}", data);
                // Usually, just confirms the connection is alive. No action needed.
            }
            Message::Close(close_frame) => {
                debug!("[WS Recv] Received Close frame: {:?}", close_frame);
                return Ok(false); // Signal to stop listening
            }
            Message::Frame(_) => {
                // Raw frame, usually handled by the library. Log if necessary.
                trace!("[WS Recv] Received raw Frame message.");
            }
        }
        Ok(true) // Continue listening
    }

    /// Handles a successfully parsed `WebSocketMessage`.
    async fn handle_parsed_message(&self, message: WebSocketMessage) {
        trace!("[WS Handle] Handling parsed message type: {}", message.message_type);
        let player_arc = match &self.player {
            Some(p) => p.clone(),
            None => {
                warn!("[WS Handle] Cannot handle '{}': Player instance not available.", message.message_type);
                return;
            }
        };

        match message.message_type.as_str() {
            "ForceKeepAlive" | "KeepAlive" => {
                // Server requesting confirmation connection is alive, or just standard keep-alive.
                debug!("[WS Handle] Received KeepAlive/ForceKeepAlive.");
                // No action needed beyond the automatic Pong responses and periodic Pings.
            }
            "GeneralCommand" => {
                if let Some(data) = message.data {
                    match serde_json::from_value::<GeneralCommand>(data) {
                        Ok(cmd) => ws_incoming_handler::handle_general_command(cmd, player_arc).await,
                        Err(e) => error!("[WS Handle] Failed to parse GeneralCommand data: {}", e),
                    }
                } else { warn!("[WS Handle] GeneralCommand message missing 'Data'."); }
            }
            "PlayState" | "Playstate" => { // Handle both potential casings
                if let Some(data) = message.data {
                     match serde_json::from_value::<PlayStateCommand>(data) {
                        Ok(cmd) => ws_incoming_handler::handle_playstate_command(cmd, player_arc).await,
                        Err(e) => error!("[WS Handle] Failed to parse PlayStateCommand data: {}", e),
                    }
                } else { warn!("[WS Handle] PlayState message missing 'Data'."); }
            }
            "Play" => {
                if let Some(data) = message.data {
                     match serde_json::from_value::<PlayCommand>(data) {
                        Ok(cmd) => ws_incoming_handler::handle_play_command(cmd, player_arc, &self.jellyfin_client).await,
                        Err(e) => error!("[WS Handle] Failed to parse PlayCommand data: {}", e),
                    }
                } else { warn!("[WS Handle] Play message missing 'Data'."); }
            }
            "UserDataChanged" => {
                // Often related to user data sync (watched status, favorites).
                // Can be complex. Log for now.
                debug!("[WS Handle] Received UserDataChanged message. Data: {:?}", message.data);
                // TODO: Implement handling if needed (e.g., update local state)
            }
            "Sessions" => {
                 // Informs about active sessions, potentially including this one.
                 debug!("[WS Handle] Received Sessions message. Data: {:?}", message.data);
                 // TODO: Implement handling if needed (e.g., update UI, check for conflicts)
            }
            "ServerRestarting" | "ServerShuttingDown" => {
                warn!("[WS Handle] Received server status message: {}. Connection will likely close.", message.message_type);
                // No action needed, the connection will likely drop.
            }
            // Add other expected message types here
            _ => {
                warn!("[WS Handle] Unhandled message type: {}", message.message_type);
                debug!("[WS Handle] Unhandled message data: {:?}", message.data);
            }
        }
    }


    /// Processes a `PlayerStateUpdate` received from the MPSC channel and sends the corresponding message over WebSocket.
    async fn process_player_update(&mut self, update: PlayerStateUpdate) -> Result<(), Box<dyn std::error::Error>> {
        debug!("[WS Update] Processing player state update: {:?}", update);

        // Lock will be acquired inside the match arms where needed

        match update {
            PlayerStateUpdate::Started { item } => {
                // PlaybackStart is now reported via HTTP POST in Player::play_current_queue_item
                debug!("[WS Update] Received PlayerStateUpdate::Started for item {}. (Reporting handled via HTTP POST)", item.id);
                // No WebSocket message needed here anymore.
            }
            PlayerStateUpdate::Stopped { item_id, final_position_ticks: _ } => { // Ignore unused field
                // PlaybackStopped is now reported via HTTP POST in Player::stop
                debug!("[WS Update] Received PlayerStateUpdate::Stopped for item {}. (Reporting handled via HTTP POST)", item_id);
                // No WebSocket message needed here anymore.
            }
            PlayerStateUpdate::Progress { item_id, position_ticks: _, .. } => { // Ignore unused field
                 // PlaybackProgress is now reported via HTTP POST by the reporter task in Player::play_current_queue_item
                 trace!("[WS Update] Received PlayerStateUpdate::Progress for item {}. (Reporting handled via HTTP POST)", item_id);
                 // No WebSocket message needed here anymore.
            }
            PlayerStateUpdate::VolumeChanged { volume, is_muted } => {
                // Volume/Mute changes are implicitly included in the periodic Progress updates.
                // We could optionally send an immediate UserDataChanged message here if needed,
                // but for now, just log it.
                debug!("[WS Update] Received VolumeChanged event (vol: {}, muted: {}). State will be reflected in next Progress report.", volume, is_muted);
            }
            // Add other PlayerStateUpdate arms if necessary (e.g., Seek, etc.)
            // _ => { // Handle other potential states if they exist
            //     trace!("[WS Update] Received unhandled PlayerStateUpdate variant: {:?}", update); // Match on outer `update`
            // }
        } // Close match update
        // Removed misplaced code block and extra closing brace from previous refactoring attempt.
        Ok(())
    }

    // Removed unused send_ws_message function (likely obsolete after reporting moved to HTTP)

    /// Sends a WebSocket Ping message to keep the connection alive.
    async fn send_keep_alive_ping(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        debug!("[WS Send] Sending periodic Ping.");
        self.websocket.send(Message::Ping(Vec::new())).await?; // Empty payload is fine
        Ok(())
    }

    /// Attempts to gracefully close the WebSocket connection.
    async fn close_websocket(&mut self) {
        debug!("[WS Close] Attempting graceful WebSocket close...");
        match self.websocket.close(None).await {
            Ok(_) => info!("WebSocket closed gracefully."),
            Err(e) => warn!("[WS Close] Error during explicit WebSocket close: {}", e),
        }
    }
}