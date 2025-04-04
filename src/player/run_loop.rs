use crate::audio::playback::AudioPlaybackControl;
use super::{Player, InternalPlayerStateUpdate, PlayerCommand, PLAYER_LOG_TARGET, command_handler};
use std::time::Duration as StdDuration;
use tokio::time::interval;
use tracing::{error, info, trace, warn};

/// Runs the player's command processing loop.
pub async fn run_player_loop(player: &mut Player) {
    info!(target: PLAYER_LOG_TARGET, "Player run loop started.");

    let mut progress_report_interval = interval(StdDuration::from_secs(5));

    loop {
        tokio::select! {
            biased; // Check commands first

            Some(command) = player.command_rx.recv() => {
                trace!(target: PLAYER_LOG_TARGET, "Received command: {:?}", command);
                match command {
                    PlayerCommand::PlayNow { item_ids, start_index } => command_handler::handle_play_now(player, item_ids, start_index).await,
                    PlayerCommand::AddToQueue { item_ids } => command_handler::handle_add_to_queue(player, item_ids).await,
                    PlayerCommand::ClearQueue => command_handler::handle_clear_queue(player).await,
                    PlayerCommand::PlayPauseToggle => command_handler::handle_play_pause_toggle(player).await,
                    PlayerCommand::Next => command_handler::handle_next(player).await,
                    PlayerCommand::Previous => command_handler::handle_previous(player).await,
                    PlayerCommand::GetFullState(responder) => {
                        let state = player.get_full_state().await;
                        let _ = responder.send(state);
                    }
                    PlayerCommand::TrackFinished => command_handler::handle_track_finished(player).await,
                    PlayerCommand::Shutdown => {
                        info!(target: PLAYER_LOG_TARGET, "Shutdown command received. Exiting run loop.");
                        let stopped_item_id = player.current_item_id.clone();
                        player.is_playing = false;
                        *player.is_paused.lock().await = false;
                        player.current_item_id = None;

                        if stopped_item_id.is_some() {
                             let _final_position = player.get_current_position().await;
                             let snapshot = player.get_playback_state_snapshot().await;
                             player.reporter.report_playback_stop(&snapshot, false).await;
                        }

                        player.broadcast_update(InternalPlayerStateUpdate::Stopped);
                        break;
                    }
                }
            }

             join_result = async { player.audio_task_manager.as_mut().unwrap().handle().await }, if player.audio_task_manager.is_some() => {
                 // Task handle completed, take ownership of the manager
                 let finished_manager = player.audio_task_manager.take().unwrap();
                 let item_id = finished_manager.item_id().to_string(); // Clone item_id for logging

                 let mut unexpected_stop = false;
                 let mut stop_reason = "unknown";

                 match join_result {
                     Ok(Ok(())) => {
                         // Task joined successfully, and the inner play() returned Ok(())
                         // This means EndOfStream or ShutdownSignal was handled internally by the audio task.
                         // The TrackFinished or Shutdown command should handle the player state update.
                         info!(target: PLAYER_LOG_TARGET, item_id = %item_id, "Audio task completed successfully (EndOfStream or Shutdown).");
                         // No state change needed here, should be handled by commands.
                     }
                     Ok(Err(ref audio_err)) => {
                          // Task joined successfully, but the inner play() returned an error
                         error!(target: PLAYER_LOG_TARGET, item_id = %item_id, "Audio task finished with internal error: {}", audio_err);
                         unexpected_stop = true;
                         stop_reason = "internal audio error";
                     }
                     Err(ref join_err) => {
                         // Task failed to join (panic, cancellation, etc.)
                         if join_err.is_panic() {
                              error!(target: PLAYER_LOG_TARGET, item_id = %item_id, "Audio task panicked: {:?}", join_err);
                              stop_reason = "task panic";
                         } else if join_err.is_cancelled() {
                              // Cancellation can happen during normal stop/clear queue, or if aborted by timeout.
                              // We might not want to treat all cancellations as "unexpected".
                              // However, if the player *thinks* it's playing, a cancellation is unexpected from its POV.
                              info!(target: PLAYER_LOG_TARGET, item_id = %item_id, "Audio task was cancelled.");
                              stop_reason = "task cancelled";
                         } else {
                              error!(target: PLAYER_LOG_TARGET, item_id = %item_id, "Audio task join error: {:?}", join_err);
                              stop_reason = "task join error";
                         }
                         unexpected_stop = true;
                     }
                 }

                 // If an unexpected stop occurred *and* the player thought it was playing, log warning and clean up state.
                 // --- Debug Logging ---
                 trace!(
                     target: PLAYER_LOG_TARGET, item_id = %item_id,
                     "Audio task handle completed. join_result_is_ok: {}, join_result_inner_is_ok: {}, unexpected_stop: {}, player.is_playing: {}",
                     join_result.is_ok(), join_result.as_ref().map(|r| r.is_ok()).unwrap_or(false), unexpected_stop, player.is_playing
                 );
                 // --- End Debug Logging ---
                 if unexpected_stop && player.is_playing {
                     warn!(target: PLAYER_LOG_TARGET, item_id = %item_id, "Audio task stopped unexpectedly (Reason: {}). Reporting stop and clearing state.", stop_reason);
                     let _final_position = player.get_current_position().await; // Get position before clearing
                     player.is_playing = false;
                     *player.is_paused.lock().await = false; // Ensure pause state is reset
                     player.current_item_id = None;
                     let snapshot = player.get_playback_state_snapshot().await;
                     player.reporter.report_playback_stop(&snapshot, false).await;
                     player.broadcast_update(InternalPlayerStateUpdate::Stopped);
                 } else if unexpected_stop {
                     // Log if it stopped unexpectedly but the player was already stopped/paused
                     trace!(target: PLAYER_LOG_TARGET, item_id = %item_id, "Audio task stopped unexpectedly (Reason: {}), but player was already stopped/paused. No state change needed.", stop_reason);
                 } else {
                     // Log if it stopped expectedly (Ok(Ok(())))
                     trace!(target: PLAYER_LOG_TARGET, item_id = %item_id, "Audio task finished expectedly. Player state should be handled by TrackFinished/Shutdown command.");
                 }
             }

             _ = progress_report_interval.tick(), if player.is_playing && !*player.is_paused.lock().await => {
                trace!(target: PLAYER_LOG_TARGET, "Progress report interval ticked.");
                let snapshot = player.get_playback_state_snapshot().await;
                player.reporter.report_playback_progress(&snapshot).await;

                if let Some(id) = player.current_item_id.clone() {
                     player.broadcast_update(InternalPlayerStateUpdate::Progress {
                         item_id: id,
                         position_ticks: player.get_current_position().await,
                     });
                }
            }

            else => {
                info!(target: PLAYER_LOG_TARGET, "Command channel closed or select! error. Exiting run loop.");
                break;
            }
        }
    }

    info!(target: PLAYER_LOG_TARGET, "Player run loop finished. Performing final cleanup.");
    if let Some(manager) = player.audio_task_manager.take() {
         info!(target: PLAYER_LOG_TARGET, "Stopping active audio task manager during final cleanup.");
         manager.stop_task().await;
    }
    info!(target: PLAYER_LOG_TARGET, "Shutting down shared audio backend...");
    match player.audio_backend.lock().await.shutdown().await {
        Ok(_) => info!(target: PLAYER_LOG_TARGET, "Shared audio backend shutdown successful."),
        Err(e) => error!(target: PLAYER_LOG_TARGET, "Error shutting down shared audio backend: {}", e),
    }
    info!(target: PLAYER_LOG_TARGET, "Player task cleanup complete.");
}