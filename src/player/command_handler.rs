use super::{Player, InternalPlayerStateUpdate, PLAYER_LOG_TARGET, item_fetcher, playback_starter};
use tracing::{info, warn, instrument};
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
    let stopped_item_id = player.current_item_id.clone();
    let _final_position = player.get_current_position().await;
    if let Some(manager) = player.audio_task_manager.take() {
        info!(target: PLAYER_LOG_TARGET, "Stopping audio task manager in handle_clear_queue.");
        manager.stop_task().await;
    }
    *player.current_progress.lock().await = PlaybackProgressInfo::default();
    player.is_playing = false;
    *player.is_paused.lock().await = false;
    player.current_item_id = None;
    if stopped_item_id.is_some() {
         let snapshot = player.get_playback_state_snapshot().await; // Needs get_playback_state_snapshot
         player.reporter.report_playback_stop(&snapshot, false).await;
    }
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
         let snapshot = player.get_playback_state_snapshot().await;
         player.reporter.report_playback_progress(&snapshot).await;
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
         let snapshot = player.get_playback_state_snapshot().await;
         player.reporter.report_playback_progress(&snapshot).await;
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

    let snapshot = player.get_playback_state_snapshot().await;
    player.reporter.report_playback_stop(&snapshot, true).await;


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
