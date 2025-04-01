use log::trace;

use futures::StreamExt;
use futures_util::SinkExt;
use serde::{Deserialize, Serialize}; // Added Serialize
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, Mutex}; // Added mpsc
use tokio::time::Duration;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};
use tokio_tungstenite::tungstenite::Message;
use url::Url;
use log::{debug, error, info, warn};

use crate::jellyfin::api::JellyfinClient;
use crate::jellyfin::models::MediaItem; // Ensure MediaItem is accessible and Clone/Serialize
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


// --- Outgoing Message Structs ---

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct PlaybackStartData {
    // Example fields (verify with Jellyfin API docs/observation):
    pub item: MediaItem, // Assuming MediaItem is Serialize + Clone
    pub can_seek: bool,
    pub position_ticks: i64,
    pub is_paused: bool,
    pub is_muted: bool,
    pub volume_level: i32,
    pub play_method: String, // e.g., "DirectPlay"
    // Add media_source_id if needed and available
    // pub media_source_id: Option<String>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct PlaybackStopData {
    // Example fields:
    pub item_id: String,
    pub position_ticks: i64,
    // Add media_source_id if needed
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct PlaybackProgressData {
    // Example fields:
    pub item_id: String,
    pub can_seek: bool,
    pub is_paused: bool,
    pub is_muted: bool,
    pub position_ticks: i64,
    pub volume_level: i32,
    pub play_method: String, // e.g., "DirectPlay"
    pub media_source_id: Option<String>, // Fill if available/needed
    pub event_name: String, // Often "TimeUpdate" or "VolumeChange" etc.
}

// UserDataChanged might be complex, using Progress for volume/mute for now
// #[derive(Debug, Serialize, Clone)]
// #[serde(rename_all = "PascalCase")]
// pub struct UserDataChangedData { ... }

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct OutgoingWsMessage<T> {
    #[serde(rename = "MessageType")]
    pub message_type: String,
    #[serde(rename = "Data")]
    pub data: T,
}

// --- Player State Update Channel ---

#[derive(Debug, Clone)]
pub enum PlayerStateUpdate {
    Started { item: MediaItem }, // Pass the full item or necessary details
    Stopped { item_id: String, final_position_ticks: i64 },
    Progress { item_id: String, position_ticks: i64, is_paused: bool, volume: i32, is_muted: bool },
    VolumeChanged { volume: i32, is_muted: bool, current_item_id: Option<String> },
    // Add other states if needed, e.g., QueueChanged
}


// --- WebSocket Handler Struct ---

pub struct WebSocketHandler {
    server_url: String,
    api_key: String,
    device_id: String,
    session_id: Option<String>,
    jellyfin_client: JellyfinClient,
    player: Option<Arc<Mutex<Player>>>,
    ws_stream: Option<WebSocketStream<MaybeTlsStream<TcpStream>>>,
    // Add the sender field
    player_update_tx: Option<mpsc::UnboundedSender<PlayerStateUpdate>>,
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
            player_update_tx: None, // Initialize as None
        }
    }

    pub fn with_session_id(mut self, session_id: &str) -> Self {
        self.session_id = Some(session_id.to_string());
        self
    }

    // Method to set the sender (called after channel creation)
    // Keep this private or ensure it's used correctly during setup
    fn set_player_update_tx(&mut self, tx: mpsc::UnboundedSender<PlayerStateUpdate>) {
        self.player_update_tx = Some(tx);
    }

    // Modify set_player to potentially update the player's sender if the channel exists
    pub fn set_player(&mut self, player: Arc<Mutex<Player>>) {
        self.player = Some(player.clone());
        // If the channel sender already exists, update the player instance
        if let Some(tx) = &self.player_update_tx {
            let tx_clone = tx.clone();
            // Spawn a task to avoid blocking while holding the handler's lock
            tokio::spawn(async move {
                 let mut player_guard = player.lock().await;
                 // Assume Player has a method like `set_update_sender`
                 // This method should ideally be part of Player::new or an init step
                 player_guard.set_update_sender(tx_clone);
                 debug!("[WS Handler] Updated player instance with state update sender.");
            });
        } else {
            debug!("[WS Handler] Player set, but update sender not yet available.");
        }
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

    // Modify prepare_for_listening to create the channel and pass the receiver
    pub fn prepare_for_listening(&mut self, shutdown_signal: Arc<AtomicBool>) -> Option<PreparedWebSocketHandler> {
        if self.ws_stream.is_none() {
            error!("Cannot prepare for listening: WebSocket not connected");
            return None;
        }

        // Create the channel here before preparing
        let (tx, rx) = mpsc::unbounded_channel::<PlayerStateUpdate>();
        self.set_player_update_tx(tx); // Store the sender in the handler

        // If player instance already exists, update its sender immediately
        if let Some(player_arc) = &self.player {
             // Ensure the tx was set above
             let tx_clone = self.player_update_tx.as_ref().expect("TX should be set").clone();
             let player_clone = Arc::clone(player_arc);
             // Spawn task to update the player instance asynchronously
             tokio::spawn(async move {
                 let mut player_guard = player_clone.lock().await;
                 // Assumes Player has `set_update_sender` method
                 player_guard.set_update_sender(tx_clone);
                 debug!("[WS Handler] Updated existing player instance with sender during prepare.");
             });
        }


        let ws_stream = self.ws_stream.take().unwrap();
        let player = self.player.clone(); // Clone Arc<Mutex<Player>>
        let jellyfin_client = self.jellyfin_client.clone();

        Some(PreparedWebSocketHandler {
            websocket: ws_stream,
            player, // Pass the Arc<Mutex<Player>>
            shutdown_signal,
            jellyfin_client,
            player_update_rx: rx, // Pass the receiver
        })
    }
}

/// PreparedWebSocketHandler is used to handle WebSocket operations without holding a mutex
pub struct PreparedWebSocketHandler {
    websocket: WebSocketStream<MaybeTlsStream<TcpStream>>,
    player: Option<Arc<Mutex<Player>>>, // Keep this to access player state if needed
    shutdown_signal: Arc<AtomicBool>,
    jellyfin_client: JellyfinClient,
    // Add the receiver field
    player_update_rx: mpsc::UnboundedReceiver<PlayerStateUpdate>,
}

impl PreparedWebSocketHandler {
    // Helper function to send outgoing messages
    async fn send_ws_message<T: Serialize>(&mut self, message_type: &str, data: T) -> Result<(), Box<dyn std::error::Error>> {
        let outgoing_message = OutgoingWsMessage {
            message_type: message_type.to_string(),
            data,
        };
        // Avoid logging potentially large data structures directly unless needed
        let json_payload = serde_json::to_string(&outgoing_message)?;
        debug!("[WS Send] Sending {} message type", message_type); // Log type only
        // For detailed debugging: trace!("[WS Send] Payload: {}", json_payload);
        self.websocket.send(Message::Text(json_payload)).await?;
        Ok(())
    }

    /// Listen for commands from the Jellyfin server and player updates
    pub async fn listen_for_commands(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        log::trace!("[WS Listen] Entered listen_for_commands");
        debug!("[WS Listen] Entering listen_for_commands function.");
        let player_arc = self.player.clone(); // Clone Arc for use in update handling
        debug!("[WS Listen] Player Arc cloned: {}", if player_arc.is_some() { "Some" } else { "None" });
        let mut ping_interval = tokio::time::interval(Duration::from_secs(30));

        debug!("[WS Listen] Ping interval created.");
        debug!("Starting WebSocket listening loop with ping every 30s");

        debug!("[WS Listen] Entering main loop..."); // Added log
        loop {
            let shutdown_state = self.shutdown_signal.load(Ordering::SeqCst);
            debug!("[WS Listen] Checking shutdown signal at loop start. Value: {}", shutdown_state); // Log the actual value
            // Shutdown signal is TRUE normally, FALSE when shutdown is requested
            if !shutdown_state {
                debug!("[WS Listen] Shutdown signal is FALSE at loop start. Breaking loop.");
                break;
            }
            
            debug!("[WS Listen] Entering select! statement..."); // Added log
            tokio::select! {
                biased; // Prioritize shutdown check slightly if needed

                // Branch 1: Message received from Jellyfin Server
                maybe_message = self.websocket.next() => {
                    debug!("[WS Listen] select! resolved: websocket.next() completed.");
                    match maybe_message {
                        Some(Ok(msg)) => {
                            if let Message::Text(text) = msg {
                                debug!("[WS] Received Text message.");
                                trace!("Received WebSocket message text: {}", text); // Use trace for full message
                                if let Ok(ws_msg) = serde_json::from_str::<WebSocketMessage>(&text) {
                                    // Pass player_arc here
                                    self.handle_message(ws_msg, player_arc.clone()).await;
                                } else {
                                    warn!("Failed to parse WebSocket message: {}", text);
                                }
                            } else if let Message::Ping(data) = msg {
                                debug!("[WS] Received Ping, sending Pong...");
                                if let Err(e) = self.websocket.send(Message::Pong(data)).await {
                                    error!("Failed to send pong: {}", e);
                                } else {
                                    debug!("[WS] Pong sent successfully.");
                                }
                            } else if msg.is_close() {
                                debug!("[WS Listen] Received Close frame from server. Exiting loop.");
                                break; // Exit loop on Close frame
                            } else {
                                debug!("[WS] Received other message type: {:?}", msg); // Log other types
                            }
                        },
                        Some(Err(e)) => {
                            debug!("[WS Listen] WebSocket read error occurred: {}. Returning Err.", e);
                            error!("WebSocket read error: {}. Attempting graceful close.", e);
                            let _ = self.websocket.close(None).await;
                            return Err(format!("WebSocket read error: {}", e).into()); // Exit function with error
                        },
                        None => {
                            debug!("[WS Listen] WebSocket stream ended (returned None). Exiting loop.");
                            break; // Exit loop if stream ends
                        }
                    }
                },

                // Branch 2: Player State Update received from Player
                Some(update) = self.player_update_rx.recv() => {
                    debug!("[WS Listen] select! resolved: player_update_rx.recv() completed.");
                    trace!("[WS Listen] Received player state update: {:?}", update); // Use trace for full update details

                    // Handle the state update by sending the appropriate message
                    match update {
                        PlayerStateUpdate::Started { item } => {
                            // Get current player state for volume/mute if possible
                            let (vol, muted) = if let Some(p_arc) = &player_arc {
                                let p_guard = p_arc.lock().await;
                                (p_guard.get_volume(), p_guard.is_muted()) // Assume these methods exist
                            } else {
                                (100, false) // Defaults if player not available
                            };

                            let data = PlaybackStartData {
                                item: item.clone(), // Ensure MediaItem is Clone + Serialize
                                can_seek: true, // Assuming seek is possible
                                position_ticks: 0,
                                is_paused: false,
                                is_muted: muted,
                                volume_level: vol,
                                play_method: "DirectPlay".to_string(),
                            };
                            if let Err(e) = self.send_ws_message("PlaybackStart", data).await {
                                error!("Failed to send PlaybackStart message: {}", e);
                            }
                        }
                        PlayerStateUpdate::Stopped { item_id, final_position_ticks } => {
                            let data = PlaybackStopData {
                                item_id,
                                position_ticks: final_position_ticks,
                            };
                            if let Err(e) = self.send_ws_message("PlaybackStopped", data).await {
                                error!("Failed to send PlaybackStopped message: {}", e);
                            }
                        }
                        // Progress update from the reporter task (contains only item_id and ticks)
                        PlayerStateUpdate::Progress { item_id, position_ticks, .. } => { // Ignore placeholder state from reporter
                            // Fetch current state from the Player instance
                            let (current_is_paused, current_volume, current_is_muted) =
                                if let Some(p_arc) = &player_arc {
                                    let p_guard = p_arc.lock().await;
                                    // Use the getter methods we added to Player
                                    (p_guard.is_paused(), p_guard.get_volume(), p_guard.is_muted())
                                } else {
                                    // Fallback if player is somehow gone (shouldn't happen ideally)
                                    warn!("[WS Listen] Player instance not available for progress update state fetch.");
                                    (false, 100, false)
                                };

                            let data = PlaybackProgressData {
                                item_id,
                                can_seek: true, // Assuming seek is generally possible
                                is_paused: current_is_paused, // Use fetched state
                                is_muted: current_is_muted,   // Use fetched state
                                position_ticks, // Use ticks from the update
                                volume_level: current_volume, // Use fetched state
                                play_method: "DirectPlay".to_string(),
                                media_source_id: None, // Fill if available/needed
                                event_name: "TimeUpdate".to_string(), // Standard progress event name
                            };

                            // Use "ReportPlaybackProgress" as message type
                            if let Err(e) = self.send_ws_message("ReportPlaybackProgress", data).await {
                                error!("Failed to send ReportPlaybackProgress message: {}", e);
                            }
                        }
                        PlayerStateUpdate::VolumeChanged { volume, is_muted, current_item_id } => {
                            // Send a Progress update to reflect volume/mute change
                            // Send a Progress update to reflect volume/mute change immediately
                            if let Some(id) = current_item_id {
                                 // Need current position and pause state
                                 let (pos_ticks, paused) = if let Some(p_arc) = &player_arc {
                                     let p_guard = p_arc.lock().await;
                                     (p_guard.get_position(), p_guard.is_paused())
                                 } else {
                                     warn!("[WS Listen] Player instance not available for VolumeChanged state fetch.");
                                     (0, false) // Defaults
                                 };

                                 let data = PlaybackProgressData {
                                     item_id: id,
                                     can_seek: true,
                                     is_paused: paused, // Use fetched state
                                     is_muted,          // Use state from update
                                     position_ticks: pos_ticks, // Use fetched state
                                     volume_level: volume,      // Use state from update
                                     play_method: "DirectPlay".to_string(),
                                     media_source_id: None,
                                     event_name: "VolumeChange".to_string(), // Indicate the event cause
                                 };
                                 if let Err(e) = self.send_ws_message("ReportPlaybackProgress", data).await {
                                     error!("Failed to send VolumeChanged (as Progress) message: {}", e);
                                 }
                             } else {
                                 warn!("VolumeChanged update received but no current item ID available. Cannot report state.");
                                 // Consider sending a general capabilities update if applicable?
                                 // Or maybe a specific SetVolume/SetMute message if the API supports it without item context?
                             }
                        }
                    }
                },

                // Branch 3: Ping interval ticked
                _ = ping_interval.tick() => {
                    debug!("[WS Listen] select! resolved: ping_interval.tick() completed.");
                    // Check shutdown signal before sending ping
                    if !self.shutdown_signal.load(Ordering::SeqCst) {
                        debug!("[WS Listen] Shutdown signal is FALSE before sending ping. Breaking loop.");
                        break;
                    }

                    debug!("Sending ping to keep WebSocket alive");
                    trace!("[WS] Sending periodic Ping..."); // Use trace for frequent messages
                    if let Err(e) = self.websocket.send(Message::Ping(vec![])).await {
                        error!("Failed to send ping: {}. Attempting graceful close.", e);
                        debug!("[WS Listen] Failed to send ping: {}. Returning Err.", e);
                        let _ = self.websocket.close(None).await;
                        return Err(format!("Failed to send ping: {}", e).into());
                    } else {
                         trace!("[WS] Ping sent successfully."); // Use trace
                    }
                }
            }
            debug!("[WS Listen] Reached end of loop iteration."); // Added log
        }

        debug!("WebSocket listener loop exited.");

        // Attempt graceful close *after* the loop has finished
        debug!("Attempting graceful WebSocket close...");
        if let Err(e) = self.websocket.close(None).await {
            warn!("Error during explicit WebSocket close: {}", e);
        } else {
            debug!("WebSocket closed gracefully.");
        }

        debug!("[WS] Exiting listening function.");
        Ok(())
    }

    // handle_message needs the player Option<Arc<Mutex<Player>>> passed in now
    async fn handle_message(&self, message: WebSocketMessage, player_option: Option<Arc<Mutex<Player>>>) {
        // Use the passed player_option directly
        if player_option.is_none() {
            warn!("Cannot handle remote control command: player not set in handler");
            return;
        }
        // Clone the Arc for passing to specific handlers if needed, avoid unwrapping here
        let player = player_option.unwrap(); // Keep unwrap for now, assuming handlers need Arc<Mutex<Player>>

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

    // These handlers now receive the Arc<Mutex<Player>> directly
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

