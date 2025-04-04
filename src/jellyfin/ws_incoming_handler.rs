//! Handles processing of specific incoming WebSocket messages from the Jellyfin server.

// Removed unused JellyfinApiContract import
use tokio::sync::mpsc;
use tracing::{debug, error, warn, instrument}; // Removed unused info

// Import the internal Player command enum
use crate::player::PlayerCommand;
// Import the Jellyfin WS command structs
use super::models_playback::{GeneralCommand, PlayCommand, PlayStateCommand};
// Removed unused MediaItem import
// Removed unused import: use crate::jellyfin::api::JellyfinClient;
// Removed unused import: use std::sync::Arc;
// Removed unused import: use tokio::sync::Mutex;
// Removed unused import: use crate::player::Player;

/// Handles "GeneralCommand" messages like SetVolume, SetMute.
#[instrument(skip(command_tx), fields(command_name = %command.name))]
pub(super) async fn handle_general_command(
    command: GeneralCommand,
    command_tx: &mpsc::Sender<PlayerCommand>,
) {
    debug!("[WS Incoming] Handling GeneralCommand: {}", command.name);

    let player_command = match command.name.as_str() {
        // "SetVolume" removed
        // "SetMute" removed
        // Add other general commands if needed (e.g., SetAudioStreamIndex, SetSubtitleStreamIndex)
        _ => {
            warn!("[WS Incoming] Unhandled GeneralCommand name: {}", command.name);
            None
        }
    };

    if let Some(cmd) = player_command {
        if let Err(e) = command_tx.send(cmd).await {
            error!("[WS Incoming] Failed to send GeneralCommand to player task: {}", e);
        }
    }
}

/// Handles "PlayState" messages like PlayPause, NextTrack, Stop, Seek.
#[instrument(skip(command_tx), fields(command_name = %command.command))]
pub(super) async fn handle_playstate_command(
    command: PlayStateCommand,
    command_tx: &mpsc::Sender<PlayerCommand>,
) {
    debug!("[WS Incoming] Handling PlayState command: {}", command.command);

    let player_command = match command.command.as_str() {
        "PlayPause" => Some(PlayerCommand::PlayPauseToggle),
        "NextTrack" => Some(PlayerCommand::Next),
        "PreviousTrack" => Some(PlayerCommand::Previous),
        // Explicit Pause/Unpause might need specific PlayerCommands if toggle isn't sufficient,
        // but for now, map them to the toggle.
        "Pause" => Some(PlayerCommand::PlayPauseToggle), // Or a specific Pause command if needed
        "Unpause" => Some(PlayerCommand::PlayPauseToggle), // Or a specific Resume command if needed
        // "Stop" / "StopMedia" removed - Stop is now implicit via other actions
        // "Seek" was never handled here, ensuring it remains unhandled.
        _ => {
            warn!("[WS Incoming] Unhandled PlayState command: {}", command.command);
            None
        }
    };

    if let Some(cmd) = player_command {
        if let Err(e) = command_tx.send(cmd).await {
            error!("[WS Incoming] Failed to send PlayStateCommand to player task: {}", e);
        }
    }
}

/// Handles "Play" messages to start or queue playback.
/// Note: Item details are fetched by the Player task now.
#[instrument(skip(command_tx), fields(command_name = %command.play_command, item_count = command.item_ids.len()))]
pub(super) async fn handle_play_command(
    command: PlayCommand,
    command_tx: &mpsc::Sender<PlayerCommand>,
    // jellyfin_client argument removed as fetching is now done in Player task
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

    // Item fetching logic removed. Player task now handles fetching based on IDs.

    // Translate to PlayerCommand and send
    let player_command = match command.play_command.as_str() {
        "PlayNow" => Some(PlayerCommand::PlayNow { item_ids: command.item_ids, start_index }),
        "PlayNext" => {
            // TODO: Implement PlayNext properly in the Player task.
            // It might require inserting at index `current_index + 1`.
            // For now, treat it like PlayLast (add to end).
            warn!("[WS Incoming] PlayNext command received, treating as PlayLast (AddToQueue) for now.");
            Some(PlayerCommand::AddToQueue { item_ids: command.item_ids })
        }
        "PlayLast" => Some(PlayerCommand::AddToQueue { item_ids: command.item_ids }),
        _ => {
            warn!("[WS Incoming] Unhandled Play command type: {}", command.play_command);
            None
        }
    };

    if let Some(cmd) = player_command {
        if let Err(e) = command_tx.send(cmd).await {
            error!("[WS Incoming] Failed to send PlayCommand to player task: {}", e);
        }
    }
}


