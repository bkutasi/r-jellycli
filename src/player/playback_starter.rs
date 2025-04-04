use crate::player::{Player, InternalPlayerStateUpdate, PLAYER_LOG_TARGET, audio_task_manager, ReportingCommand}; // Added ReportingCommand
use crate::jellyfin::models::MediaItem;
use crate::jellyfin::reporter::{report_playback_start, report_playback_progress, PlaybackStartInfoBody, PlaybackProgressInfoBody}; // Import needed items
use crate::audio::PlaybackProgressInfo;
use tokio::sync::mpsc; // Added for channel
use tracing::{debug, error, info, instrument, trace, warn}; // Added warn
use crate::player::{run_reporting_task, stop_and_clear_reporting_task}; // Import from player mod now

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

    // --- Update Player State ---
    player.is_playing = true;
    player.current_item_id = Some(item_to_play.id.clone());
    // Note: is_paused was already set to false earlier

    // --- Report Playback Started to Jellyfin ---
    // Send this *before* starting the progress reporting task
    let start_info = PlaybackStartInfoBody {
        item_id: item_to_play.id.clone(),
        queue_item_id: None, // TODO: Add if queue context is available/needed
        can_seek: true, // Assume seekable for now
        is_muted: player.is_muted,
        is_paused: *player.is_paused.lock().await, // Should be false here
        volume_level: player.volume_level,
    };
    if let Err(e) = report_playback_start(player.jellyfin_client.as_ref(), &start_info).await {
        error!(target: PLAYER_LOG_TARGET, item_id = %item_to_play.id, "Failed to report playback start: {}", e);
        // Continue anyway, but log the error
    } else {
        // Also send an initial progress report with 0 ticks immediately after start
        debug!(target: PLAYER_LOG_TARGET, item_id = %item_to_play.id, "Sending initial 0-tick progress report.");
        let progress_info = PlaybackProgressInfoBody {
            item_id: item_to_play.id.clone(),
            queue_item_id: None,
            can_seek: true,
            is_muted: player.is_muted,
            is_paused: false, // Should be false here
            volume_level: player.volume_level,
            position_ticks: 0,
            event_name: "timeupdate".to_string(), // Use "timeupdate" for consistency
        };
        if let Err(e) = report_playback_progress(player.jellyfin_client.as_ref(), &progress_info).await {
             warn!(target: PLAYER_LOG_TARGET, item_id = %item_to_play.id, "Failed to send initial 0-tick progress report: {}", e);
        }
    }

    // --- Stop Previous Reporting Task (if any) ---
    stop_and_clear_reporting_task(player).await;

    // --- Spawn New Reporting Task ---
    let (report_cmd_tx, report_cmd_rx) = mpsc::channel::<ReportingCommand>(32); // Buffer size can be adjusted
    let initial_state = player.get_full_state().await; // Get state *after* updating is_playing etc.

    // Clone the Arc<dyn JellyfinApiContract> for the task
    let api_arc_clone = player.jellyfin_client.clone();

    let reporting_handle = tokio::spawn(run_reporting_task(
        api_arc_clone, // Pass the cloned Arc directly
        initial_state.clone(), // Clone initial state for the task
        report_cmd_rx,
        player.current_progress.clone(), // Pass the SharedProgress Arc
    ));

    // Store the sender and handle
    player.reporting_command_tx = Some(report_cmd_tx.clone()); // Clone sender for potential immediate use
    player.reporting_task_handle = Some(reporting_handle);

    // Send initial state update to the new task
    if let Err(e) = report_cmd_tx.send(ReportingCommand::StateUpdate(initial_state)).await {
         error!(target: PLAYER_LOG_TARGET, "Failed to send initial state update to new reporting task: {}", e);
         // Consider stopping the task again if the initial send fails?
    }

    // --- Broadcast Player State Updates ---
    // (Keep the existing broadcast updates for UI/other listeners)
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
    // Stop previous audio task *and* reporting task
    if player.audio_task_manager.is_some() {
        info!(target: PLAYER_LOG_TARGET, "Stopping previous audio and reporting tasks before playing new item.");
        if let Some(manager) = player.audio_task_manager.take() {
            manager.stop_task().await;
        }
        stop_and_clear_reporting_task(player).await; // Stop reporting task as well
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