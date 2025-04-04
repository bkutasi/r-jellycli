//! Handles the WebSocket connection to the Jellyfin server for real-time communication.
//! Manages sending player state updates and receiving remote control commands.

use tracing::instrument;
use crate::jellyfin::models_playback::{GeneralCommand, PlayCommand, PlayStateCommand};
use futures::{SinkExt, StreamExt};
use tracing::{debug, error, info, trace, warn};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tokio::{net::TcpStream, sync::{broadcast, mpsc}};
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};
use tokio_tungstenite::tungstenite::Message;
use url::Url;

use crate::jellyfin::api::JellyfinClient;
use crate::player::{InternalPlayerStateUpdate, PlayerCommand};

use super::ws_incoming_handler;

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


// --- WebSocket Handler Setup ---

/// Manages the initial setup and connection of the WebSocket.
pub struct WebSocketHandler {
    server_url: String,
    api_key: String,
    device_id: String,
    ws_stream: Option<WebSocketStream<MaybeTlsStream<TcpStream>>>,
    shutdown_tx: broadcast::Sender<()>,
}

impl WebSocketHandler {
    /// Creates a new WebSocketHandler instance.
    pub fn new(
        _jellyfin_client: JellyfinClient,
        server_url: &str,
        api_key: &str,
        device_id: &str,
        shutdown_tx: broadcast::Sender<()>,
    ) -> Self {
        WebSocketHandler {
            server_url: server_url.trim_end_matches('/').to_string(),
            api_key: api_key.to_string(),
            device_id: device_id.to_string(),
            ws_stream: None,
            shutdown_tx,
        }
    }


    /// Establishes the WebSocket connection to the Jellyfin server.
    #[instrument(skip(self), fields(device_id = %self.device_id))]
    pub async fn connect(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let parsed_url = Url::parse(&self.server_url)?;
        let scheme = if parsed_url.scheme() == "https" { "wss" } else { "ws" };
        let host = parsed_url.host_str().ok_or("Server URL missing host")?;
        let port_str = parsed_url.port().map_or_else(
            || if scheme == "wss" { ":443".to_string() } else { ":80".to_string() },
            |p| format!(":{}", p),
        );
        // Use default port only if not explicitly specified in the URL
        let host_port = format!("{}{}", host, if parsed_url.port().is_some() { port_str } else { "".to_string() });


        let path = parsed_url.path();

        // Construct WebSocket URL exactly like jellycli:
        // {scheme}://{host}:{port}{path}/socket?api_key={api_key}&deviceId={device_id}
        let ws_url_str = format!(
            "{}://{}{}/socket?api_key={}&deviceId={}",
            scheme,
            host_port,
            if path == "/" { "" } else { path },
            self.api_key,
            self.device_id
        );

        info!("Attempting WebSocket connection to: {}...", host_port);
        debug!("[WS Connect] Full WebSocket URL: {}", ws_url_str);

        let ws_url = Url::parse(&ws_url_str)?;
        let (ws_stream, response) = connect_async(ws_url).await?;

        debug!("[WS Connect] WebSocket connection established. Response status: {}", response.status());
        info!("WebSocket connected successfully to {}", host_port);

        let mut temp_ws_stream = ws_stream;

        // Send initial KeepAlive immediately after connecting (required by Jellyfin)
        let keep_alive_msg = r#"{"MessageType": "KeepAlive"}"#;
        debug!("[WS Connect] Sending initial KeepAlive message.");
        trace!("[WS Connect] KeepAlive payload: {}", keep_alive_msg);
        if let Err(e) = temp_ws_stream.send(Message::Text(keep_alive_msg.to_string())).await {
            error!("[WS Connect] Failed to send initial KeepAlive message: {}", e);
            return Err(format!("Failed to send initial KeepAlive: {}", e).into());
        }
        debug!("[WS Connect] Initial KeepAlive message sent successfully.");

        self.ws_stream = Some(temp_ws_stream);

        Ok(())
    }


    /// Prepares the handler for the listening loop.
    /// Takes ownership of the WebSocket stream and requires the necessary channels.
    pub fn prepare_for_listening(
        &mut self,
        player_command_tx: mpsc::Sender<PlayerCommand>,
        player_state_rx: broadcast::Receiver<InternalPlayerStateUpdate>,
        shutdown_rx: broadcast::Receiver<()>,
    ) -> Option<PreparedWebSocketHandler> {
        let ws_stream = match self.ws_stream.take() {
            Some(stream) => stream,
            None => {
                error!("[WS Prepare] Cannot prepare for listening: WebSocket not connected or already taken.");
                return None;
            }
        };


        debug!("[WS Prepare] Prepared for listening. Handing over stream and receiver.");
        Some(PreparedWebSocketHandler {
            websocket: ws_stream,
            // player: self.player.clone(), // Removed direct player access
            player_command_tx,
            shutdown_rx,
            player_state_rx,
            shutdown_tx: self.shutdown_tx.clone(),
        })

    }
}

// --- Active WebSocket Listener ---

/// Handles the active WebSocket listening loop, processing incoming messages
/// and sending outgoing state updates. Owns the WebSocket stream and MPSC receiver.
pub struct PreparedWebSocketHandler {
    websocket: WebSocketStream<MaybeTlsStream<TcpStream>>,
    player_command_tx: mpsc::Sender<PlayerCommand>,
    shutdown_rx: broadcast::Receiver<()>,
    player_state_rx: broadcast::Receiver<InternalPlayerStateUpdate>,
    #[allow(dead_code)] // TODO: Remove allow when shutdown logic is fully implemented
    shutdown_tx: broadcast::Sender<()>,
}

impl PreparedWebSocketHandler {
    /// Listens for commands from the Jellyfin server and player updates.
    /// This is the main loop for the active WebSocket connection.
    #[instrument(skip_all, name = "ws_listener_loop")]
    pub async fn listen_for_commands(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        info!("Starting WebSocket listening loop...");
        let mut ping_interval = tokio::time::interval(Duration::from_secs(30));

        loop {

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
                            let _ = self.websocket.close(None).await;
                            return Err(format!("WebSocket read error: {}", e).into());
                        },
                        None => {
                            info!("WebSocket stream ended (returned None). Exiting loop.");
                            break;
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
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            warn!("[WS Listen] Player state update receiver lagged by {} messages. Some state changes might have been missed.", n);
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            info!("Player state update channel closed. Exiting loop.");
                            break;
                        }
                    }
                },

                // --- Branch 3: Ping Interval ---
                _ = ping_interval.tick() => {
                    trace!("[WS Listen] select! resolved: ping_interval.tick()");
                    if let Err(e) = self.send_keep_alive_ping().await {
                         error!("[WS Listen] Failed to send WebSocket ping: {}. Returning error.", e);
                         let error_to_return = e;
                         return Err(error_to_return);
                    }
                },

                // --- Branch 4: Shutdown Signal ---
                _ = self.shutdown_rx.recv() => {
                    trace!("[WS Listen] select! resolved: shutdown_rx.recv()");
                    info!("Shutdown signal received via broadcast channel. Exiting loop.");
                    break;
                }
            }
            trace!("[WS Listen] Reached end of loop iteration.");
        }

        info!("Listener loop exited.");
        self.close_websocket().await;
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
            }
            Message::Ping(data) => {
                debug!("[WS Recv] Received Ping from server.");
                trace!("[WS Recv] Ping payload: {:?}", data);
                if let Err(e) = self.websocket.send(Message::Pong(data)).await {
                    error!("[WS Send] Failed to send Pong response: {}", e);
                } else {
                    debug!("[WS Send] Pong response sent successfully.");
                }
            }
            Message::Pong(data) => {
                debug!("[WS Recv] Received Pong from server.");
                trace!("[WS Recv] Pong payload: {:?}", data);
            }
            Message::Close(close_frame) => {
                debug!("[WS Recv] Received Close frame: {:?}", close_frame);
                return Ok(false);
            }
            Message::Frame(_) => {
                trace!("[WS Recv] Received raw Frame message.");
            }
        }
        Ok(true)
    }

    /// Handles a successfully parsed `WebSocketMessage` by translating it to a PlayerCommand.
    async fn handle_parsed_message(&mut self, message: WebSocketMessage) {
        trace!("[WS Handle] Handling parsed message type: {}", message.message_type);

        match message.message_type.as_str() {
            "ForceKeepAlive" | "KeepAlive" => {
                debug!("[WS Handle] Received KeepAlive/ForceKeepAlive.");
            }
            "GeneralCommand" => {
                if let Some(data) = message.data {
                    match serde_json::from_value::<GeneralCommand>(data) {
                        Ok(cmd) => ws_incoming_handler::handle_general_command(cmd, &self.player_command_tx).await,
                        Err(e) => error!("[WS Handle] Failed to parse GeneralCommand data: {}", e),
                    }
                } else { warn!("[WS Handle] GeneralCommand message missing 'Data'."); }
            }
            "PlayState" | "Playstate" => {
                if let Some(data) = message.data {
                     match serde_json::from_value::<PlayStateCommand>(data) {
                        Ok(cmd) => ws_incoming_handler::handle_playstate_command(cmd, &self.player_command_tx).await,
                        Err(e) => error!("[WS Handle] Failed to parse PlayStateCommand data: {}", e),
                    }
                } else { warn!("[WS Handle] PlayState message missing 'Data'."); }
            }
            "Play" => {
                if let Some(data) = message.data {
                     match serde_json::from_value::<PlayCommand>(data) {
                        Ok(cmd) => ws_incoming_handler::handle_play_command(cmd, &self.player_command_tx).await,
                        Err(e) => error!("[WS Handle] Failed to parse PlayCommand data: {}", e),
                    }
                } else { warn!("[WS Handle] Play message missing 'Data'."); }
            }
            "UserDataChanged" => {
                debug!("[WS Handle] Received UserDataChanged message. Data: {:?}", message.data);
            }
            "Sessions" => {
                 debug!("[WS Handle] Received Sessions message. Data: {:?}", message.data);
            }
            "ServerRestarting" | "ServerShuttingDown" => {
                warn!("[WS Handle] Received server status message: {}. Connection will likely close.", message.message_type);
            }
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
            }
            InternalPlayerStateUpdate::Paused { item, .. } => {
                debug!("[WS Update] State: Paused item {}", item.id);
            }
            InternalPlayerStateUpdate::Stopped => {
                debug!("[WS Update] State: Stopped");
            }
            InternalPlayerStateUpdate::Progress { item_id, .. } => {
                trace!("[WS Update] State: Progress for item {}", item_id);
            }
            InternalPlayerStateUpdate::QueueChanged { queue_ids, current_index } => {
                debug!("[WS Update] State: QueueChanged ({} items, index {})", queue_ids.len(), current_index);
            }
             InternalPlayerStateUpdate::Error(err_msg) => {
                 error!("[WS Update] Received player error state: {}", err_msg);
            }
        }
        Ok(())
    }

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
                Err(Box::new(e))
            }
        }
    }


    /// Sends a WebSocket Ping message to keep the connection alive.
    async fn send_keep_alive_ping(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        debug!("[WS Send] Sending periodic Ping.");
        self.websocket.send(Message::Ping(Vec::new())).await?;
        Ok(())
    }

    async fn close_websocket(&mut self) {
        debug!("[WS Close] Attempting graceful WebSocket close...");
        match self.websocket.close(None).await {
            Ok(_) => info!("WebSocket closed gracefully."),
            Err(e) => warn!("[WS Close] Error during explicit WebSocket close: {}", e),
        }
    }
}