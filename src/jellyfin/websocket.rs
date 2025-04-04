//! Handles the WebSocket connection to the Jellyfin server for real-time communication.
//! Manages sending player state updates and receiving remote control commands.

// Removed unused imports: PlaybackStartReport, PlaybackStopReport
use tracing::instrument;
use crate::jellyfin::models_playback::{GeneralCommand, PlayCommand, PlayStateCommand}; // Import directly
use futures::{SinkExt, StreamExt};
use tracing::{debug, error, info, trace, warn}; // Replaced log with tracing
use serde::{Deserialize, Serialize};
// Removed unused imports: AtomicBool, Ordering
// Removed unused import: use std::sync::Arc;
use std::time::Duration;
use tokio::{net::TcpStream, sync::{broadcast, mpsc}}; // Removed Mutex import
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};
use tokio_tungstenite::tungstenite::Message;
use url::Url;

use crate::jellyfin::api::JellyfinClient;
// Import the internal player types
use crate::player::{InternalPlayerStateUpdate, PlayerCommand};

use super::ws_incoming_handler; // Import the incoming handler module

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

// PlayerStateUpdate enum removed, using InternalPlayerStateUpdate from player.rs now.

// --- WebSocket Handler Setup ---

/// Manages the initial setup and connection of the WebSocket.
pub struct WebSocketHandler {
    server_url: String,
    api_key: String,
    device_id: String,
    // jellyfin_client: JellyfinClient, // Removed - Unused field
    // player: Option<Arc<Mutex<Player>>>, // Remove direct player access
    ws_stream: Option<WebSocketStream<MaybeTlsStream<TcpStream>>>,
    // player_update_tx: Option<mpsc::UnboundedSender<PlayerStateUpdate>>, // Remove MPSC sender
    shutdown_tx: broadcast::Sender<()>, // Add shutdown sender
    // session_id is not used for the initial connection based on jellycli behavior
    // session_id: Option<String>,
}

impl WebSocketHandler {
    /// Creates a new WebSocketHandler instance.
    pub fn new(
        _jellyfin_client: JellyfinClient, // Parameter kept for signature compatibility if needed, but marked unused
        server_url: &str,
        api_key: &str,
        device_id: &str,
        shutdown_tx: broadcast::Sender<()>, // Add shutdown_tx parameter
    ) -> Self {
        WebSocketHandler {
            server_url: server_url.trim_end_matches('/').to_string(), // Ensure no trailing slash
            api_key: api_key.to_string(),
            device_id: device_id.to_string(),
            // jellyfin_client, // Removed initialization
            // player: None, // Removed
            ws_stream: None,
            // player_update_tx: None, // Removed
            shutdown_tx, // Store the sender
        }
    }

    // pub fn with_session_id(mut self, session_id: &str) -> Self {
    //     self.session_id = Some(session_id.to_string());
    //     self
    // }

    // Removed set_player method as direct player access is removed.
    // The player command sender and state receiver are passed during setup/preparation.

    // Removed is_player_set method.
    // Removed set_player_update_tx method.

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


    /// Prepares the handler for the listening loop.
    /// Takes ownership of the WebSocket stream and requires the necessary channels.
    pub fn prepare_for_listening(
        &mut self,
        player_command_tx: mpsc::Sender<PlayerCommand>, // Sender for commands TO player
        player_state_rx: broadcast::Receiver<InternalPlayerStateUpdate>, // Receiver for state FROM player
        shutdown_rx: broadcast::Receiver<()>, // Receiver for app shutdown
    ) -> Option<PreparedWebSocketHandler> {
        let ws_stream = match self.ws_stream.take() {
            Some(stream) => stream,
            None => {
                error!("[WS Prepare] Cannot prepare for listening: WebSocket not connected or already taken.");
                return None;
            }
        };

        // MPSC channel creation removed. State updates come via broadcast receiver.

        debug!("[WS Prepare] Prepared for listening. Handing over stream and receiver.");
        Some(PreparedWebSocketHandler {
            websocket: ws_stream,
            // player: self.player.clone(), // Removed direct player access
            player_command_tx, // Store command sender
            shutdown_rx, // Store shutdown receiver
            // jellyfin_client: self.jellyfin_client.clone(), // Removed - no longer needed here
            player_state_rx, // Store state update receiver
            shutdown_tx: self.shutdown_tx.clone(), // Store shutdown sender clone
        })

    }
} // End of impl WebSocketHandler

// --- Active WebSocket Listener ---

/// Handles the active WebSocket listening loop, processing incoming messages
/// and sending outgoing state updates. Owns the WebSocket stream and MPSC receiver.
pub struct PreparedWebSocketHandler {
    websocket: WebSocketStream<MaybeTlsStream<TcpStream>>,
    // player: Option<Arc<Mutex<Player>>>, // Removed direct player access
    player_command_tx: mpsc::Sender<PlayerCommand>, // Sender for commands TO player
    shutdown_rx: broadcast::Receiver<()>, // Receiver for app shutdown
    // jellyfin_client: JellyfinClient, // Removed - no longer needed here
    player_state_rx: broadcast::Receiver<InternalPlayerStateUpdate>, // Receiver for state FROM player
    #[allow(dead_code)] // TODO: Remove allow when shutdown logic is fully implemented
    shutdown_tx: broadcast::Sender<()>, // Sender for app shutdown (used by incoming handler)
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

                // --- Branch 2: Player State Update from Player (Broadcast Receiver) ---
                update_result = self.player_state_rx.recv() => {
                    trace!("[WS Listen] select! resolved: player_state_rx.recv()");
                    match update_result {
                        Ok(update) => {
                            if let Err(e) = self.process_player_update(update).await {
                                error!("[WS Listen] Failed to process player update: {}", e);
                                // Decide if this error is fatal. Continuing for now.
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            warn!("[WS Listen] Player state update receiver lagged by {} messages. Some state changes might have been missed.", n);
                            // Continue listening, but log the lag.
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            info!("Player state update channel closed. Exiting loop.");
                            break; // Exit if the sender side is dropped
                        }
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

    /// Handles a successfully parsed `WebSocketMessage` by translating it to a PlayerCommand.
    async fn handle_parsed_message(&mut self, message: WebSocketMessage) {
        trace!("[WS Handle] Handling parsed message type: {}", message.message_type);
        // No longer need direct player access here. Commands are sent via channel.

        match message.message_type.as_str() {
            "ForceKeepAlive" | "KeepAlive" => {
                // Server requesting confirmation connection is alive, or just standard keep-alive.
                debug!("[WS Handle] Received KeepAlive/ForceKeepAlive.");
                // No action needed beyond the automatic Pong responses and periodic Pings.
            }
            "GeneralCommand" => {
                if let Some(data) = message.data {
                    match serde_json::from_value::<GeneralCommand>(data) {
                        // Pass the command sender to the handler
                        Ok(cmd) => ws_incoming_handler::handle_general_command(cmd, &self.player_command_tx).await,
                        Err(e) => error!("[WS Handle] Failed to parse GeneralCommand data: {}", e),
                    }
                } else { warn!("[WS Handle] GeneralCommand message missing 'Data'."); }
            }
            "PlayState" | "Playstate" => { // Handle both potential casings
                if let Some(data) = message.data {
                     match serde_json::from_value::<PlayStateCommand>(data) {
                        // Pass the command sender to the handler
                        Ok(cmd) => ws_incoming_handler::handle_playstate_command(cmd, &self.player_command_tx).await,
                        Err(e) => error!("[WS Handle] Failed to parse PlayStateCommand data: {}", e),
                    }
                } else { warn!("[WS Handle] PlayState message missing 'Data'."); }
            }
            "Play" => {
                if let Some(data) = message.data {
                     match serde_json::from_value::<PlayCommand>(data) {
                        // Pass the command sender and temporarily the client/player for item fetching
                        // jellyfin_client argument removed as fetching is now done in Player task
                        Ok(cmd) => ws_incoming_handler::handle_play_command(cmd, &self.player_command_tx).await,
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


    /// Processes an `InternalPlayerStateUpdate` received from the broadcast channel.
    /// Sends corresponding messages over WebSocket if applicable (most state is reported via HTTP POST now).
    async fn process_player_update(&mut self, update: InternalPlayerStateUpdate) -> Result<(), Box<dyn std::error::Error>> {
        debug!("[WS Update] Processing player state update: {:?}", update);

        // Most state updates (Start, Stop, Progress, Volume) are reported via HTTP POST by the Player task.
        // This handler primarily needs to react to state changes that might require *sending* a specific
        // WebSocket message *from* the client to the server, if any exist.
        // Based on common Jellyfin client behavior, most actions are client-initiated or reported via POST.
        // We might send `UserDataChanged` on volume change, or `QueueChanged` if the queue structure changes.

        match update {
            InternalPlayerStateUpdate::Playing { item, .. } => {
                debug!("[WS Update] State: Playing item {}", item.id);
                // Reporting handled via HTTP POST by Player.
            }
            InternalPlayerStateUpdate::Paused { item, .. } => {
                debug!("[WS Update] State: Paused item {}", item.id);
                // Reporting handled via HTTP POST by Player.
            }
            InternalPlayerStateUpdate::Stopped => {
                debug!("[WS Update] State: Stopped");
                // Reporting handled via HTTP POST by Player.
            }
            InternalPlayerStateUpdate::Progress { item_id, .. } => {
                trace!("[WS Update] State: Progress for item {}", item_id);
                // Reporting handled via HTTP POST by Player.
            }
            InternalPlayerStateUpdate::QueueChanged { queue_ids, current_index } => {
                debug!("[WS Update] State: QueueChanged ({} items, index {})", queue_ids.len(), current_index);
                // TODO: Does Jellyfin WS require a specific message for queue changes initiated by the client?
                // If so, construct and send it here. Example (replace with actual message if needed):
                // let queue_data = ...; // Format queue data
                // let msg = OutgoingWsMessage { message_type: "QueueChanged".to_string(), data: queue_data };
                // self.send_ws_message(&msg).await?;
            }
            // InternalPlayerStateUpdate::VolumeChanged removed as the variant no longer exists
             InternalPlayerStateUpdate::Error(err_msg) => {
                 error!("[WS Update] Received player error state: {}", err_msg);
                 // Optionally send an error notification via WS if the protocol supports it.
            }
        }
        Ok(())
    }

    /// Helper to serialize and send an OutgoingWsMessage.
    /// Helper to serialize and send an OutgoingWsMessage.
    #[allow(dead_code)] // TODO: Remove allow when messages are sent from here
    async fn send_ws_message<T: Serialize + std::fmt::Debug>(&mut self, message: &OutgoingWsMessage<T>) -> Result<(), Box<dyn std::error::Error>> {
        match serde_json::to_string(message) {
            Ok(json_payload) => {
                debug!("[WS Send] Sending message: Type='{}'", message.message_type);
                trace!("[WS Send] Payload: {}", json_payload);
                self.websocket.send(Message::Text(json_payload)).await?;
                Ok(())
            }
            Err(e) => {
                error!("[WS Send] Failed to serialize outgoing message ({:?}): {}", message, e);
                Err(Box::new(e)) // Propagate serialization error
            }
        }
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