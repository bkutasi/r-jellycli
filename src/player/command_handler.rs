use super::{Player, InternalPlayerStateUpdate, PLAYER_LOG_TARGET, item_fetcher, playback_starter, ReportingCommand}; // Import ReportingCommand
// VolumeChanged and MuteStatusChanged are variants of InternalPlayerStateUpdate
use tracing::{info, warn, instrument, debug, trace}; // Add trace
use crate::audio::PlaybackProgressInfo;


#[instrument(skip(player, item_ids), fields(item_count = item_ids.len(), start_index = start_index))]
pub async fn handle_play_now(player: &mut Player, item_ids: Vec<String>, start_index: usize) {
    info!(target: PLAYER_LOG_TARGET, "Handling PlayNow command with {} items, starting at index {}.", item_ids.len(), start_index);

    // --- Fetch Item Details ---
    let fetched_items = match item_fetcher::fetch_and_sort_items(player.jellyfin_client.clone(), &item_ids).await {
        Ok(items) => items,
        Err(e) => {
            player.broadcast_update(InternalPlayerStateUpdate::Error(format!("Failed to fetch item details: {}", e)));
            return;
        }
    };

    let items_to_play = fetched_items.into_iter().skip(start_index).collect::<Vec<_>>();
    if items_to_play.is_empty() {
        warn!(target: PLAYER_LOG_TARGET, "PlayNow: No items left after applying start_index {}.", start_index);
        handle_clear_queue(player).await;
        return;
    }

    handle_clear_queue(player).await;
    player.queue = items_to_play;
    player.broadcast_update(InternalPlayerStateUpdate::QueueChanged {
        queue_ids: player.queue.iter().map(|i| i.id.clone()).collect(),
        current_index: 0,
    });


    if !player.queue.is_empty() {
        player.current_queue_index = 0;
        playback_starter::play_current_item(player, false).await;
    }
}

#[instrument(skip(player, item_ids), fields(item_count = item_ids.len()))]
pub async fn handle_add_to_queue(player: &mut Player, item_ids: Vec<String>) {
    if item_ids.is_empty() {
        return;
    }
    info!(target: PLAYER_LOG_TARGET, "Handling AddToQueue command with {} item IDs.", item_ids.len());

    let fetched_items = match item_fetcher::fetch_and_sort_items(player.jellyfin_client.clone(), &item_ids).await {
        Ok(items) => items,
        Err(e) => {
             player.broadcast_update(InternalPlayerStateUpdate::Error(format!("Failed to fetch item details: {}", e)));
             return;
        }
    };

    if fetched_items.is_empty() {
         warn!(target: PLAYER_LOG_TARGET, "AddToQueue: No valid items found after fetching details.");
         return;
    }

    let was_empty = player.queue.is_empty();
    player.queue.extend(fetched_items);
    player.broadcast_update(InternalPlayerStateUpdate::QueueChanged {
        queue_ids: player.queue.iter().map(|i| i.id.clone()).collect(),
        current_index: player.current_queue_index,
    });

    if was_empty && !player.queue.is_empty() && !player.is_playing {
        info!(target: PLAYER_LOG_TARGET, "AddToQueue: Queue was empty, starting playback automatically.");
        player.current_queue_index = 0;
        playback_starter::play_current_item(player, false).await;
    }
}

#[instrument(skip(player))]
pub async fn handle_clear_queue(player: &mut Player) {
    info!(target: PLAYER_LOG_TARGET, "Handling ClearQueue command.");
    let _stopped_item_id = player.current_item_id.clone(); // Prefix unused variable
    let _final_position = player.get_current_position().await;
    if let Some(manager) = player.audio_task_manager.take() {
        info!(target: PLAYER_LOG_TARGET, "Stopping audio task manager in handle_clear_queue.");
        manager.stop_task().await;
    }
    *player.current_progress.lock().await = PlaybackProgressInfo::default();
    player.is_playing = false;
    *player.is_paused.lock().await = false;
    player.current_item_id = None;
    // Stop reporting task is handled by run_loop when audio_task_manager finishes or is stopped.
    // if stopped_item_id.is_some() {
    //      let snapshot = player.get_playback_state_snapshot().await;
    //      // player.reporter.report_playback_stop(&snapshot, false).await; // Removed
    // }
    player.broadcast_update(InternalPlayerStateUpdate::Stopped);

    player.queue.clear();
    player.current_queue_index = 0;
    player.broadcast_update(InternalPlayerStateUpdate::QueueChanged {
        queue_ids: Vec::new(),
        current_index: 0,
    });
}

#[instrument(skip(player))]
pub async fn handle_play_pause_toggle(player: &mut Player) {
    info!(target: PLAYER_LOG_TARGET, "Handling PlayPauseToggle command.");
    if !player.is_playing {
        if !player.queue.is_empty() {
            playback_starter::play_current_item(player, false).await;
        } else {
            warn!(target: PLAYER_LOG_TARGET, "PlayPauseToggle: Queue is empty, cannot start playback.");
        }
    } else {
        let currently_paused = *player.is_paused.lock().await;
        if currently_paused {
            handle_resume(player).await;
        } else {
            handle_pause(player).await;
        }
    }
}

#[instrument(skip(player))]
pub async fn handle_pause(player: &mut Player) {
    if !player.is_playing || *player.is_paused.lock().await {
        return;
    }
    info!(target: PLAYER_LOG_TARGET, "Handling Pause command.");
    *player.is_paused.lock().await = true;
    if let Some(item) = player.queue.get(player.current_queue_index).cloned() {
         player.broadcast_update(InternalPlayerStateUpdate::Paused {
             item,
             position_ticks: player.get_current_position().await,
             queue_ids: player.queue.iter().map(|i| i.id.clone()).collect(),
             queue_index: player.current_queue_index,
         });
         // Send state update to the reporting task
         let snapshot = player.get_full_state().await;
         send_reporting_command(player, ReportingCommand::StateUpdate(snapshot)).await;
    }
}

 #[instrument(skip(player))]
pub async fn handle_resume(player: &mut Player) {
    if !player.is_playing || !*player.is_paused.lock().await {
        return;
    }
    info!(target: PLAYER_LOG_TARGET, "Handling Resume command.");
    *player.is_paused.lock().await = false;
    if let Some(item) = player.queue.get(player.current_queue_index).cloned() {
         player.broadcast_update(InternalPlayerStateUpdate::Playing {
             item,
             position_ticks: player.get_current_position().await,
             queue_ids: player.queue.iter().map(|i| i.id.clone()).collect(),
             queue_index: player.current_queue_index,
         });
         // Send state update to the reporting task
         let snapshot = player.get_full_state().await;
         send_reporting_command(player, ReportingCommand::StateUpdate(snapshot)).await;
    }
}


#[instrument(skip(player))]
pub async fn handle_next(player: &mut Player) {
    info!(target: PLAYER_LOG_TARGET, "Handling Next command.");
    if player.queue.is_empty() || player.current_queue_index >= player.queue.len() - 1 {
        info!(target: PLAYER_LOG_TARGET, "Next: Already at end of queue or queue empty.");
        return;
    }
    player.current_queue_index += 1;
    playback_starter::play_current_item(player, true).await;
}

#[instrument(skip(player))]
pub async fn handle_previous(player: &mut Player) {
    info!(target: PLAYER_LOG_TARGET, "Handling Previous command.");
    if player.queue.is_empty() || player.current_queue_index == 0 {
        info!(target: PLAYER_LOG_TARGET, "Previous: Already at start of queue or queue empty.");
        return;
    }
    player.current_queue_index -= 1;
    playback_starter::play_current_item(player, true).await;
}


#[instrument(skip(player), fields(is_handling = player.is_handling_track_finish))]
pub async fn handle_track_finished(player: &mut Player) {
    // --- State Lock ---
    if player.is_handling_track_finish {
        warn!(target: PLAYER_LOG_TARGET, "TrackFinished received while already handling a previous finish. Ignoring.");
        return;
    }
    player.is_handling_track_finish = true;
    // --- End State Lock ---

    info!(target: PLAYER_LOG_TARGET, "Handling TrackFinished internal command.");
    let _final_position = player.get_current_position().await;

    // Stop reporting task is handled by run_loop when audio_task_manager finishes.
    // let snapshot = player.get_playback_state_snapshot().await;
    // player.reporter.report_playback_stop(&snapshot, true).await; // Removed


    player.is_playing = false;
    *player.is_paused.lock().await = false;
    player.current_item_id = None;


    if player.current_queue_index < player.queue.len() - 1 {
        player.current_queue_index += 1;
        info!(target: PLAYER_LOG_TARGET, "Track finished, advancing to index {}.", player.current_queue_index);
        playback_starter::play_current_item(player, true).await;
        player.is_handling_track_finish = false; // Release lock after starting next
    } else {
        info!(target: PLAYER_LOG_TARGET, "Track finished, end of queue reached.");
        player.broadcast_update(InternalPlayerStateUpdate::Stopped);
        player.is_handling_track_finish = false; // Release lock at end of queue
    }
}


#[instrument(skip(player), fields(new_volume = volume))]
pub async fn handle_set_volume(player: &mut Player, volume: u32) {
    let clamped_volume = volume.min(100); // Clamp volume to 0-100
    info!(target: PLAYER_LOG_TARGET, "Handling SetVolume command: {}", clamped_volume);
    player.volume_level = clamped_volume; // Assumes Player struct has volume_level: u32

    // Also unmute if volume is set > 0
    if clamped_volume > 0 && player.is_muted {
        player.is_muted = false;
        player.broadcast_update(InternalPlayerStateUpdate::MuteStatusChanged { is_muted: false }); // Use full path
        debug!(target: PLAYER_LOG_TARGET, "Unmuted due to volume change.");
        // Send state update after potential mute change
        let snapshot = player.get_full_state().await;
        send_reporting_command(player, ReportingCommand::StateUpdate(snapshot.clone())).await; // Clone snapshot if needed below
    }

    player.broadcast_update(InternalPlayerStateUpdate::VolumeChanged { volume_level: clamped_volume }); // Use full path
    // Send state update after volume change
    // If mute status wasn't changed above, we still need to send the update
    if !(clamped_volume > 0 && player.is_muted) { // Check if mute status was already handled
       let snapshot = player.get_full_state().await;
       send_reporting_command(player, ReportingCommand::StateUpdate(snapshot)).await;
    }

    // Apply volume to the audio backend if it's running
    // TODO: Implement set_volume on AudioTaskManager and uncomment this block
    // if let Some(manager) = &player.audio_task_manager {
    //     if let Err(e) = manager.set_volume(clamped_volume as f32 / 100.0).await {
    //          warn!(target: PLAYER_LOG_TARGET, "Failed to apply volume to audio backend: {}", e);
    //     }
    // }
}

#[instrument(skip(player))]
pub async fn handle_toggle_mute(player: &mut Player) {
    player.is_muted = !player.is_muted; // Assumes Player struct has is_muted: bool
    info!(target: PLAYER_LOG_TARGET, "Handling ToggleMute command. New state: {}", player.is_muted);
    player.broadcast_update(InternalPlayerStateUpdate::MuteStatusChanged { is_muted: player.is_muted });
    // Send state update after mute toggle
    let snapshot = player.get_full_state().await;
    send_reporting_command(player, ReportingCommand::StateUpdate(snapshot)).await;

    // Apply mute state to the audio backend if it's running
    if let Some(manager) = &player.audio_task_manager {
         if let Err(e) = manager.set_mute(player.is_muted).await {
             warn!(target: PLAYER_LOG_TARGET, "Failed to apply mute state to audio backend: {}", e); // Corrected format string
         }
    }
}

/// Helper to send a command to the reporting task if it's active.
async fn send_reporting_command(player: &Player, command: ReportingCommand) {
     if let Some(sender) = &player.reporting_command_tx {
        trace!(target: PLAYER_LOG_TARGET, "Sending command to reporting task: {:?}", command);
        if let Err(e) = sender.send(command).await {
            warn!(target: PLAYER_LOG_TARGET, "Failed to send command to reporting task (channel closed?): {}", e);
            // Consider if the sender/handle should be cleared here if sending fails repeatedly.
        }
    } else {
         trace!(target: PLAYER_LOG_TARGET, "Cannot send command, no active reporting task sender: {:?}", command);
    }
}
