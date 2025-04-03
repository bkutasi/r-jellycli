//! Handles processing of specific incoming WebSocket messages from the Jellyfin server.

use std::sync::Arc;
use tokio::sync::{broadcast, Mutex}; // Add broadcast
use tracing::{debug, error, info, warn}; // Replaced log with tracing
use tracing::instrument;

use crate::jellyfin::api::JellyfinClient;
use crate::jellyfin::models::MediaItem;
use crate::player::Player;

use super::models_playback::{GeneralCommand, PlayCommand, PlayStateCommand}; // Use specific types

/// Handles "GeneralCommand" messages like SetVolume, ToggleMute.
#[instrument(skip(_player), fields(command_name = %command.name))]
pub(super) async fn handle_general_command(
    command: GeneralCommand,
    _player: Arc<Mutex<Player>>,
) {
    debug!("[WS Incoming] Handling GeneralCommand: {}", command.name);

    match command.name.as_str() {
        // Removed SetVolume handler
        // Removed ToggleMute handler
        // Add other general commands if needed (e.g., SetAudioStreamIndex, SetSubtitleStreamIndex)
        _ => {
            warn!("[WS Incoming] Unhandled GeneralCommand name: {}", command.name);
        }
    }
}

/// Handles "PlayState" messages like PlayPause, NextTrack, Stop.
#[instrument(skip(player), fields(command_name = %command.command))]
pub(super) async fn handle_playstate_command(
    command: PlayStateCommand,
    player: Arc<Mutex<Player>>,
    shutdown_tx: &broadcast::Sender<()>, // Add shutdown sender parameter
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
            info!("[WS Incoming] Stop command received. Stopping playback and clearing queue.");
            let mut player_guard = player.lock().await;
            player_guard.stop().await; // Stop playback first
            player_guard.clear_queue().await; // Then clear the queue
        
            // Send shutdown signal
            info!("[WS Incoming] Sending shutdown signal after Stop command.");
            if let Err(e) = shutdown_tx.send(()) {
                error!("[WS Incoming] Failed to send shutdown signal: {}", e);
                // Log error, but don't prevent the rest of the stop logic
            }
        }
        "Seek" => {
            if let Some(ticks) = command.seek_position_ticks {
                debug!("[WS Incoming] Seek to {} ticks", ticks);
                let mut player_guard = player.lock().await;
                // Assuming Player::seek takes ticks directly
                player_guard.seek(ticks).await;
            } else {
                warn!("[WS Incoming] Seek command received without SeekPositionTicks.");
            }
        }
        _ => {
            warn!("[WS Incoming] Unhandled PlayState command: {}", command.command);
        }
    }
}

/// Handles "Play" messages to start or queue playback.
#[instrument(skip(player, jellyfin_client), fields(command_name = %command.play_command, item_count = command.item_ids.len()))]
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
                    debug!("[WS Incoming] PlayNow: Stopping playback, clearing queue, and adding items.");
                    // Lock -> Stop -> Clear -> Add -> Play -> Unlock (implicitly)
                    let mut player_guard = player.lock().await;
                    player_guard.stop().await; // Explicitly stop current playback first
                    player_guard.clear_queue().await; // Clear queue state
                    player_guard.add_items(items_to_process); // Add new items
                    player_guard.play_from_start().await; // Start playing the new queue
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