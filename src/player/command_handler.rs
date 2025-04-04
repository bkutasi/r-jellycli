// src/player/command_handler.rs
use super::{Player, InternalPlayerStateUpdate, PLAYER_LOG_TARGET, item_fetcher, playback_starter}; // Removed unused PlayerCommand
// use crate::jellyfin::models::MediaItem; // Unused
// use crate::jellyfin::api::JellyfinError; // Unused
use tracing::{info, warn, instrument}; // Removed unused debug, error
// use std::sync::Arc; // Unused
// use tokio::sync::{Mutex as TokioMutex, broadcast}; // Unused
use crate::audio::PlaybackProgressInfo; // Import needed type
// use tokio::task::JoinHandle; // Unused

// --- Command Handlers ---

#[instrument(skip(player, item_ids), fields(item_count = item_ids.len(), start_index = start_index))]
pub async fn handle_play_now(player: &mut Player, item_ids: Vec<String>, start_index: usize) {
    info!(target: PLAYER_LOG_TARGET, "Handling PlayNow command with {} items, starting at index {}.", item_ids.len(), start_index);

    // --- Fetch Item Details ---
    // Need access to fetch_and_sort_items, which is currently on Player
    // Call the function from the item_fetcher module
    let fetched_items = match item_fetcher::fetch_and_sort_items(player.jellyfin_client.clone(), &item_ids).await {
        Ok(items) => items,
        Err(e) => {
            // Broadcast the error since the fetcher doesn't do it anymore
            player.broadcast_update(InternalPlayerStateUpdate::Error(format!("Failed to fetch item details: {}", e)));
            return;
        }
    };

    // --- Apply Start Index ---
    let items_to_play = fetched_items.into_iter().skip(start_index).collect::<Vec<_>>();
    if items_to_play.is_empty() {
        warn!(target: PLAYER_LOG_TARGET, "PlayNow: No items left after applying start_index {}.", start_index);
        handle_clear_queue(player).await; // Ensure queue is clear if nothing remains
        return;
    }

    // --- Clear Queue and Add New Items ---
    handle_clear_queue(player).await; // Stop playback and clear queue first
    player.queue = items_to_play; // Replace queue with the processed items
    player.broadcast_update(InternalPlayerStateUpdate::QueueChanged {
        queue_ids: player.queue.iter().map(|i| i.id.clone()).collect(),
        current_index: 0, // Will start at 0
    });


    // --- Start Playback ---
    if !player.queue.is_empty() {
        player.current_queue_index = 0; // Reset index for the new queue
        playback_starter::play_current_item(player, false).await; // Use playback_starter
    }
}

#[instrument(skip(player, item_ids), fields(item_count = item_ids.len()))]
pub async fn handle_add_to_queue(player: &mut Player, item_ids: Vec<String>) {
    if item_ids.is_empty() {
        return;
    }
    info!(target: PLAYER_LOG_TARGET, "Handling AddToQueue command with {} item IDs.", item_ids.len());

    // --- Fetch Item Details ---
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

    // --- Add to Queue ---
    let was_empty = player.queue.is_empty();
    player.queue.extend(fetched_items);
    player.broadcast_update(InternalPlayerStateUpdate::QueueChanged {
        queue_ids: player.queue.iter().map(|i| i.id.clone()).collect(),
        current_index: player.current_queue_index,
    });

    // --- Auto-play if queue was empty and now isn't ---
    if was_empty && !player.queue.is_empty() && !player.is_playing {
        info!(target: PLAYER_LOG_TARGET, "AddToQueue: Queue was empty, starting playback automatically.");
        player.current_queue_index = 0; // Start from the beginning of the newly added items
        playback_starter::play_current_item(player, false).await; // Use playback_starter
    }
}

#[instrument(skip(player))]
pub async fn handle_clear_queue(player: &mut Player) {
    info!(target: PLAYER_LOG_TARGET, "Handling ClearQueue command.");
    // Stop playback implicitly by stopping the backend
    let stopped_item_id = player.current_item_id.clone();
    let _final_position = player.get_current_position().await; // Prefix unused variable
    // Stop the audio task manager if it exists
    if let Some(manager) = player.audio_task_manager.take() {
        info!(target: PLAYER_LOG_TARGET, "Stopping audio task manager in handle_clear_queue.");
        manager.stop_task().await;
    }
    // Reset progress after stopping
    *player.current_progress.lock().await = PlaybackProgressInfo::default();
    player.is_playing = false;
    *player.is_paused.lock().await = false;
    player.current_item_id = None;
    // Report stop if something was playing
    if stopped_item_id.is_some() {
         // Create snapshot *before* clearing state further
         let snapshot = player.get_playback_state_snapshot().await; // Needs get_playback_state_snapshot
         player.reporter.report_playback_stop(&snapshot, false).await; // Use reporter
    }
    player.broadcast_update(InternalPlayerStateUpdate::Stopped); // Broadcast internal stop

    // Now clear queue
    player.queue.clear();
    player.current_queue_index = 0;
    // player.current_item_id = None; // Already cleared above
    player.broadcast_update(InternalPlayerStateUpdate::QueueChanged {
        queue_ids: Vec::new(),
        current_index: 0,
    });
}

#[instrument(skip(player))]
pub async fn handle_play_pause_toggle(player: &mut Player) {
    info!(target: PLAYER_LOG_TARGET, "Handling PlayPauseToggle command.");
    if !player.is_playing {
        // If stopped, start playing from the current index (or 0 if queue was empty)
        if !player.queue.is_empty() {
            playback_starter::play_current_item(player, false).await; // Use playback_starter
        } else {
            warn!(target: PLAYER_LOG_TARGET, "PlayPauseToggle: Queue is empty, cannot start playback.");
        }
    } else {
        // If playing, toggle pause state
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
        return; // Already paused or stopped
    }
    info!(target: PLAYER_LOG_TARGET, "Handling Pause command.");
    // Directly update the shared pause state. The audio task will react to this.
    *player.is_paused.lock().await = true;
    // Broadcast the state change
    if let Some(item) = player.queue.get(player.current_queue_index).cloned() {
         player.broadcast_update(InternalPlayerStateUpdate::Paused {
             item,
             position_ticks: player.get_current_position().await,
             queue_ids: player.queue.iter().map(|i| i.id.clone()).collect(),
             queue_index: player.current_queue_index,
         });
         // Report progress immediately after pause/resume for quick feedback
         let snapshot = player.get_playback_state_snapshot().await;
         player.reporter.report_playback_progress(&snapshot).await; // Use reporter
    }
}

 #[instrument(skip(player))]
pub async fn handle_resume(player: &mut Player) {
    if !player.is_playing || !*player.is_paused.lock().await {
        return; // Already playing or stopped
    }
    info!(target: PLAYER_LOG_TARGET, "Handling Resume command.");
    // Directly update the shared pause state. The audio task will react to this.
    *player.is_paused.lock().await = false;
    // Broadcast the state change
    if let Some(item) = player.queue.get(player.current_queue_index).cloned() {
         player.broadcast_update(InternalPlayerStateUpdate::Playing {
             item,
             position_ticks: player.get_current_position().await,
             queue_ids: player.queue.iter().map(|i| i.id.clone()).collect(),
             queue_index: player.current_queue_index,
         });
         // Report progress immediately after pause/resume for quick feedback
         let snapshot = player.get_playback_state_snapshot().await;
         player.reporter.report_playback_progress(&snapshot).await; // Use reporter
    }
}


// handle_stop removed - stop is implicit via other commands or shutdown
#[instrument(skip(player))]
pub async fn handle_next(player: &mut Player) {
    info!(target: PLAYER_LOG_TARGET, "Handling Next command.");
    if player.queue.is_empty() || player.current_queue_index >= player.queue.len() - 1 {
        info!(target: PLAYER_LOG_TARGET, "Next: Already at end of queue or queue empty.");
        // Optionally stop playback if at the end? Current behavior is no-op.
        return;
    }
    player.current_queue_index += 1;
    playback_starter::play_current_item(player, true).await; // Use playback_starter
}

#[instrument(skip(player))]
pub async fn handle_previous(player: &mut Player) {
    info!(target: PLAYER_LOG_TARGET, "Handling Previous command.");
    // TODO: Add logic to restart current track if position > threshold (e.g., 3 seconds)
    if player.queue.is_empty() || player.current_queue_index == 0 {
        info!(target: PLAYER_LOG_TARGET, "Previous: Already at start of queue or queue empty.");
        return;
    }
    player.current_queue_index -= 1;
    playback_starter::play_current_item(player, true).await; // Use playback_starter
}

// handle_set_volume removed
// handle_set_mute removed

#[instrument(skip(player))]
pub async fn handle_track_finished(player: &mut Player) {
    info!(target: PLAYER_LOG_TARGET, "Handling TrackFinished internal command.");
    // let _finished_item_id = player.current_item_id.clone(); // Unused
    let _final_position = player.get_current_position().await; // Prefix unused variable

    // Report completion *before* stopping backend/advancing state
    // Note: stop_audio_backend is called implicitly by the audio task finishing,
    // so we don't call it explicitly here. We just need to update our state.
    // Create snapshot *before* clearing state further
    let snapshot = player.get_playback_state_snapshot().await;
    player.reporter.report_playback_stop(&snapshot, true).await; // Use reporter, true = completed naturally

    // Clear the task handle and shutdown sender as the task finished itself
    // Task manager is already cleared implicitly when the task finishes or is stopped.
    // No need to clear player.audio_task_manager here.

    // Reset playing state
    player.is_playing = false;
    *player.is_paused.lock().await = false;
    player.current_item_id = None; // Clear current item ID

    // TODO: Implement repeat modes here
    // If repeat single, replay current index
    // If repeat all, wrap around queue

    // Advance to next track if available
    if player.current_queue_index < player.queue.len() - 1 {
        player.current_queue_index += 1;
        info!(target: PLAYER_LOG_TARGET, "Track finished, advancing to index {}.", player.current_queue_index);
        playback_starter::play_current_item(player, true).await; // Use playback_starter
    } else {
        info!(target: PLAYER_LOG_TARGET, "Track finished, end of queue reached.");
        // Reached end of queue, stop playback completely
        // State is already updated above. Broadcast Stopped.
        player.broadcast_update(InternalPlayerStateUpdate::Stopped);
    }
}

// play_current_item function moved to playback_starter module