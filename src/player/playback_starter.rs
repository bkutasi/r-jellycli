// src/player/playback_starter.rs
use crate::player::{Player, PlayerCommand, InternalPlayerStateUpdate, PLAYER_LOG_TARGET, audio_task_manager}; // Use re-exported types
use crate::jellyfin::models::MediaItem;
use crate::audio::PlaybackProgressInfo;
// use tokio::sync::broadcast; // Unused
use tracing::{debug, error, info, instrument};

/// Result type for prepare_playback
struct PlaybackPreparation {
    item_to_play: MediaItem,
    stream_url: String,
}

/// Handles validating the queue index and fetching the stream URL.
#[instrument(skip(player), fields(queue_index = player.current_queue_index))]
async fn prepare_playback(player: &mut Player) -> Result<PlaybackPreparation, ()> { // Return empty tuple on handled error
    // --- Validate Queue Index ---
    let item_to_play = match player.queue.get(player.current_queue_index) {
        Some(item) => item.clone(),
        None => {
            error!(target: PLAYER_LOG_TARGET, "Cannot play item at index {}: Index out of bounds.", player.current_queue_index);
            player.broadcast_update(InternalPlayerStateUpdate::Error("Invalid queue index".to_string()));
            // Ensure player state reflects stop if index is invalid
            player.is_playing = false;
            *player.is_paused.lock().await = false;
            player.current_item_id = None;
            // Don't broadcast stopped here, let the caller handle the final state
            return Err(()); // Indicate handled error
        }
    };
    info!(target: PLAYER_LOG_TARGET, "Preparing to play item: {} ({})", item_to_play.name, item_to_play.id);

    // --- Get Stream URL ---
    let stream_url = match player.jellyfin_client.get_audio_stream_url(&item_to_play.id).await {
        Ok(url) => {
            debug!(target: PLAYER_LOG_TARGET, "Got stream URL: {}", url);
            url
        },
        Err(e) => {
            error!(target: PLAYER_LOG_TARGET, "Failed to get stream URL for {}: {}", item_to_play.id, e);
            player.broadcast_update(InternalPlayerStateUpdate::Error(format!("Failed to get stream URL: {}", e)));
             // Ensure player state reflects stop if URL fetch fails
            player.is_playing = false;
            *player.is_paused.lock().await = false;
            player.current_item_id = None;
            // Don't broadcast stopped here
            return Err(()); // Indicate handled error
        }
    };

    Ok(PlaybackPreparation { item_to_play, stream_url })
}

/// Spawns the audio task and updates player state.
#[instrument(skip(player, backend, item_to_play, stream_url), fields(item_id = %item_to_play.id))]
async fn initiate_audio_task(
    player: &mut Player,
    backend: std::sync::Arc<tokio::sync::Mutex<crate::audio::PlaybackOrchestrator>>, // Use the shared backend type
    item_to_play: MediaItem,
    stream_url: String,
) {
    // --- Reset State (Progress already reset before calling prepare_playback) ---
    *player.is_paused.lock().await = false;

    // --- Create Finish Callback ---
    let internal_cmd_tx = player.internal_command_tx.clone();
    let item_id_for_callback = item_to_play.id.clone(); // Clone ID for the callback closure
    let on_finish_callback: Box<dyn FnOnce() + Send + Sync + 'static> = Box::new(move || {
        info!(target: PLAYER_LOG_TARGET, item_id=%item_id_for_callback, "Audio task finished track naturally. Sending TrackFinished command.");
        if let Err(e) = internal_cmd_tx.try_send(PlayerCommand::TrackFinished) {
             error!(target: PLAYER_LOG_TARGET, item_id=%item_id_for_callback, "Failed to send TrackFinished command: {}", e);
        }
    });

    // --- Spawn Task using AudioTaskManager ---
    let manager = audio_task_manager::spawn_playback_task(
        backend, // Move backend ownership
        stream_url.clone(),
        item_to_play.id.clone(),
        item_to_play.run_time_ticks,
        on_finish_callback,
        player.internal_command_tx.clone(),
    );

    // --- Store Task Manager ---
    player.audio_task_manager = Some(manager);

    // --- Update Player State Immediately ---
    player.is_playing = true;
    player.current_item_id = Some(item_to_play.id.clone());

    // --- Report Playback Start ---
    let snapshot = player.get_playback_state_snapshot().await;
    player.reporter.report_playback_start(&snapshot).await;

    // --- Broadcast State Updates ---
    player.broadcast_update(InternalPlayerStateUpdate::Playing {
        item: item_to_play, // Move item ownership
        position_ticks: 0,
        queue_ids: player.queue.iter().map(|i| i.id.clone()).collect(),
        queue_index: player.current_queue_index,
    });
     player.broadcast_update(InternalPlayerStateUpdate::QueueChanged {
        queue_ids: player.queue.iter().map(|i| i.id.clone()).collect(),
        current_index: player.current_queue_index,
    });
}

/// Plays the item at the current queue index.
/// Stops any existing playback first.
#[instrument(skip(player), fields(queue_index = player.current_queue_index))]
pub async fn play_current_item(player: &mut Player, _is_track_change: bool) {
    // Stop any existing playback task *before* starting a new one.
    if let Some(manager) = player.audio_task_manager.take() {
        info!(target: PLAYER_LOG_TARGET, "Stopping previous audio task before playing new item.");
        manager.stop_task().await;
    }
    // Reset progress regardless of whether a task was running
    *player.current_progress.lock().await = PlaybackProgressInfo::default();


    // Prepare playback (validate index, get URL)
    match prepare_playback(player).await {
        Ok(preparation) => {
            // Get a clone of the shared audio backend Arc
            let shared_backend = player.audio_backend.clone();
            // Initiate the audio task, passing the shared backend
            initiate_audio_task(player, shared_backend, preparation.item_to_play, preparation.stream_url).await;
        }
        Err(_) => {
            // Error already handled and broadcasted within prepare_playback
            // Ensure the final state is Stopped if preparation failed
            player.broadcast_update(InternalPlayerStateUpdate::Stopped);
            info!(target: PLAYER_LOG_TARGET, "Playback preparation failed, ensuring Stopped state.");
        }
    }
}