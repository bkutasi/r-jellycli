//! Handles processing of specific incoming WebSocket messages from the Jellyfin server.

use tokio::sync::mpsc;
use tracing::{debug, error, warn, instrument};

use crate::player::PlayerCommand;
use super::models_playback::{GeneralCommand, PlayCommand, PlayStateCommand};

/// Handles "GeneralCommand" messages like SetVolume, SetMute.
#[instrument(skip(command_tx), fields(command_name = %command.name))]
pub(super) async fn handle_general_command(
    command: GeneralCommand,
    command_tx: &mpsc::Sender<PlayerCommand>,
) {
    debug!("[WS Incoming] Handling GeneralCommand: {}", command.name);

    let player_command = match command.name.as_str() {
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
        "Pause" => Some(PlayerCommand::PlayPauseToggle),
        "Unpause" => Some(PlayerCommand::PlayPauseToggle),
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


    let player_command = match command.play_command.as_str() {
        "PlayNow" => Some(PlayerCommand::PlayNow { item_ids: command.item_ids, start_index }),
        "PlayNext" => {
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


