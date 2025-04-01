//! Handles processing of specific incoming WebSocket messages from the Jellyfin server.

use std::sync::Arc;
use tokio::sync::Mutex;
use log::{debug, error, info, warn};

use crate::jellyfin::api::JellyfinClient;
use crate::jellyfin::models::MediaItem;
use crate::player::Player;

use super::models_playback::{GeneralCommand, PlayCommand, PlayStateCommand}; // Use specific types

/// Handles "GeneralCommand" messages like SetVolume, ToggleMute.
pub(super) async fn handle_general_command(
    command: GeneralCommand,
    player: Arc<Mutex<Player>>,
) {
    debug!("[WS Incoming] Handling GeneralCommand: {}", command.name);

    match command.name.as_str() {
        "SetVolume" => {
            if let Some(arguments) = &command.arguments {
                if let Some(vol_val) = arguments.get("Volume") {
                    if let Some(vol_u64) = vol_val.as_u64() {
                        match u8::try_from(vol_u64) {
                            Ok(vol) if vol <= 100 => {
                                debug!("[WS Incoming] SetVolume to {}", vol);
                                { // Scope for the lock guard
                                    let mut player_guard = player.lock().await;
                                    player_guard.set_volume(vol).await; // Assuming Player implements this
                                } // Guard dropped here
                            }
                            Ok(vol) => {
                                warn!("[WS Incoming] Received SetVolume with value > 100: {}", vol);
                            }
                            Err(_) => {
                                error!("[WS Incoming] Failed to convert SetVolume value to u8: {:?}", vol_val);
                            }
                        }
                    } else {
                        warn!("[WS Incoming] SetVolume 'Volume' argument is not a valid number: {:?}", vol_val);
                    }
                } else {
                    warn!("[WS Incoming] SetVolume command missing 'Volume' argument.");
                }
            } else {
                warn!("[WS Incoming] SetVolume command missing arguments field.");
            }
        }
        "ToggleMute" => {
            debug!("[WS Incoming] ToggleMute");
            { // Scope for the lock guard
                let mut player_guard = player.lock().await;
                player_guard.toggle_mute().await; // Assuming Player implements this
            } // Guard dropped here
        }
        // Add other general commands if needed (e.g., SetAudioStreamIndex, SetSubtitleStreamIndex)
        _ => {
            warn!("[WS Incoming] Unhandled GeneralCommand name: {}", command.name);
        }
    }
}

/// Handles "PlayState" messages like PlayPause, NextTrack, Stop.
pub(super) async fn handle_playstate_command(
    command: PlayStateCommand,
    player: Arc<Mutex<Player>>,
) {
    debug!("[WS Incoming] Handling PlayState command: {}", command.command);

    // Lock is acquired and dropped within each match arm to avoid holding across awaits
    match command.command.as_str() {
        "PlayPause" => {
            debug!("[WS Incoming] PlayPause");
            let mut player_guard = player.lock().await;
            player_guard.play_pause().await;
        }
        "NextTrack" => {
            debug!("[WS Incoming] NextTrack");
            let mut player_guard = player.lock().await;
            player_guard.next().await;
        }
        "PreviousTrack" => {
            debug!("[WS Incoming] PreviousTrack");
            let mut player_guard = player.lock().await;
            player_guard.previous().await;
        }
        "Pause" => {
            debug!("[WS Incoming] Pause");
            let mut player_guard = player.lock().await;
            player_guard.pause().await;
        }
        "Unpause" => {
            debug!("[WS Incoming] Unpause / Resume");
            let mut player_guard = player.lock().await;
            player_guard.resume().await;
        }
        "Stop" | "StopMedia" => { // Handle both potential names
            debug!("[WS Incoming] Stop");
            let mut player_guard = player.lock().await;
            player_guard.stop().await;
        }
        "Seek" => {
            // TODO: Implement Seek handling if needed
            // Requires parsing "SeekPositionTicks" from arguments (likely in GeneralCommand format?)
            warn!("[WS Incoming] Seek command received but not implemented.");
        }
        _ => {
            warn!("[WS Incoming] Unhandled PlayState command: {}", command.command);
        }
    }
}

/// Handles "Play" messages to start or queue playback.
pub(super) async fn handle_play_command(
    command: PlayCommand,
    player: Arc<Mutex<Player>>,
    jellyfin_client: &JellyfinClient, // Pass as reference
) {
    debug!(
        "[WS Incoming] Handling Play command: '{}' with {} items, start index {}",
        command.play_command,
        command.item_ids.len(),
        command.start_index.unwrap_or(0)
    );

    if command.item_ids.is_empty() {
        warn!("[WS Incoming] Received Play command with empty ItemIds list.");
        return;
    }

    let start_index = command.start_index.unwrap_or(0) as usize;
    if start_index >= command.item_ids.len() {
        warn!(
            "[WS Incoming] Play command start_index {} is out of bounds for {} items.",
            start_index,
            command.item_ids.len()
        );
        return;
    }

    // Fetch item details
    info!(
        "[WS Incoming] Fetching details for {} items for Play command...",
        command.item_ids.len()
    );
    match jellyfin_client.get_items_details(&command.item_ids).await {
        Ok(mut media_items) => {
            info!(
                "[WS Incoming] Successfully fetched details for {} items.",
                media_items.len()
            );

            // Ensure the order matches the request, as the API might not guarantee it.
            // Create a map for quick lookup of original positions.
            let original_order: std::collections::HashMap<&String, usize> = command
                .item_ids
                .iter()
                .enumerate()
                .map(|(i, id)| (id, i))
                .collect();

            media_items.sort_by_key(|item| *original_order.get(&item.id).unwrap_or(&usize::MAX));

            // Apply start_index
            let items_to_process: Vec<MediaItem> =
                media_items.into_iter().skip(start_index).collect();

            if items_to_process.is_empty() {
                warn!(
                    "[WS Incoming] No items left to process after applying start_index {}.",
                    start_index
                );
                return;
            }

            debug!(
                "[WS Incoming] Items to process ({}): {:?}",
                items_to_process.len(),
                items_to_process.iter().map(|i| i.id.clone()).collect::<Vec<_>>()
            );

            // Lock the player only when needed and drop the guard before awaits if possible,
            // or ensure the lock is held only for the duration of the specific async call.
            match command.play_command.as_str() {
                "PlayNow" => {
                    debug!("[WS Incoming] PlayNow: Clearing queue and adding items.");
                    // Lock -> Clear -> Add -> Play -> Unlock (implicitly)
                    let mut player_guard = player.lock().await;
                    player_guard.clear_queue().await; // Assuming this is quick or handles its own async locking internally
                    player_guard.add_items(items_to_process); // Assuming this is synchronous
                    player_guard.play_from_start().await; // Assuming this handles its own async locking internally
                }
                "PlayNext" => {
                    debug!("[WS Incoming] PlayNext: Inserting items at the beginning of the queue.");
                    // Requires a Player method like `add_items_next`
                    // let mut player_guard = player.lock().await;
                    // player_guard.add_items_next(items_to_process); // Assuming sync
                    warn!("[WS Incoming] PlayNext command received but not implemented in Player.");
                }
                "PlayLast" => {
                    debug!("[WS Incoming] PlayLast: Appending items to the end of the queue.");
                    let mut player_guard = player.lock().await;
                    player_guard.add_items(items_to_process); // Assumes add_items appends (sync)
                }
                _ => {
                    warn!(
                        "[WS Incoming] Unhandled Play command type: {}",
                        command.play_command
                    );
                }
            }
        }
        Err(e) => {
            error!("[WS Incoming] Failed to fetch item details for Play command: {}", e);
            // Consider sending feedback to the server if possible/necessary
        }
    }
}