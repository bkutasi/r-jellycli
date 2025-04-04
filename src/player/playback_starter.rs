use crate::player::{Player, InternalPlayerStateUpdate, PLAYER_LOG_TARGET, audio_task_manager};
use crate::jellyfin::models::MediaItem;
use crate::audio::PlaybackProgressInfo;
use tracing::{debug, error, info, instrument, trace};

struct PlaybackPreparation {
    item_to_play: MediaItem,
    stream_url: String,
}

/// Handles validating the queue index and fetching the stream URL.
#[instrument(skip(player), fields(queue_index = player.current_queue_index))]
async fn prepare_playback(player: &mut Player) -> Result<PlaybackPreparation, ()> {
    let item_to_play = match player.queue.get(player.current_queue_index) {
        Some(item) => item.clone(),
        None => {
            error!(target: PLAYER_LOG_TARGET, "Cannot play item at index {}: Index out of bounds.", player.current_queue_index);
            player.broadcast_update(InternalPlayerStateUpdate::Error("Invalid queue index".to_string()));
            player.is_playing = false;
            *player.is_paused.lock().await = false;
            player.current_item_id = None;
            return Err(());
        }
    };
    info!(target: PLAYER_LOG_TARGET, "Preparing to play item: {} ({})", item_to_play.name, item_to_play.id);

    let stream_url = match player.jellyfin_client.get_audio_stream_url(&item_to_play.id).await {
        Ok(url) => {
            debug!(target: PLAYER_LOG_TARGET, "Got stream URL: {}", url);
            url
        },
        Err(e) => {
            error!(target: PLAYER_LOG_TARGET, "Failed to get stream URL for {}: {}", item_to_play.id, e);
            player.broadcast_update(InternalPlayerStateUpdate::Error(format!("Failed to get stream URL: {}", e)));
            player.is_playing = false;
            *player.is_paused.lock().await = false;
            player.current_item_id = None;
            return Err(());
        }
    };

    Ok(PlaybackPreparation { item_to_play, stream_url })
}

/// Spawns the audio task and updates player state.
#[instrument(skip(player, backend, item_to_play, stream_url), fields(item_id = %item_to_play.id))]
async fn initiate_audio_task(
    player: &mut Player,
    backend: std::sync::Arc<tokio::sync::Mutex<crate::audio::PlaybackOrchestrator>>,
    item_to_play: MediaItem,
    stream_url: String,
) {
    *player.is_paused.lock().await = false;

    let item_id_for_callback = item_to_play.id.clone();
    // This callback is passed to the audio task manager.
    // The audio task manager's *internal* callback is now solely responsible
    // for sending the TrackFinished command upon natural completion.
    // We keep this Box::new(|| {}) structure in case other logic needs
    // to be added here later, but it no longer sends the command itself.
    let on_finish_callback: Box<dyn FnOnce() + Send + Sync + 'static> = Box::new(move || {
        trace!(target: PLAYER_LOG_TARGET, item_id=%item_id_for_callback, "Placeholder on_finish_callback executed (no command sent).");
    });

    let manager = audio_task_manager::spawn_playback_task(
        backend,
        stream_url.clone(),
        item_to_play.id.clone(),
        item_to_play.run_time_ticks,
        on_finish_callback,
        player.internal_command_tx.clone(),
    );

    player.audio_task_manager = Some(manager);

    player.is_playing = true;
    player.current_item_id = Some(item_to_play.id.clone());

    let snapshot = player.get_playback_state_snapshot().await;
    player.reporter.report_playback_start(&snapshot).await;

    player.broadcast_update(InternalPlayerStateUpdate::Playing {
        item: item_to_play,
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
#[instrument(skip(player), fields(queue_index = player.current_queue_index))]
pub async fn play_current_item(player: &mut Player, _is_track_change: bool) {
    if let Some(manager) = player.audio_task_manager.take() {
        info!(target: PLAYER_LOG_TARGET, "Stopping previous audio task before playing new item.");
        manager.stop_task().await;
    }
    *player.current_progress.lock().await = PlaybackProgressInfo::default();


    match prepare_playback(player).await {
        Ok(preparation) => {
            let shared_backend = player.audio_backend.clone();
            initiate_audio_task(player, shared_backend, preparation.item_to_play, preparation.stream_url).await;
        }
        Err(_) => {
            player.broadcast_update(InternalPlayerStateUpdate::Stopped);
            info!(target: PLAYER_LOG_TARGET, "Playback preparation failed, ensuring Stopped state.");
        }
    }
}