// src/player/run_loop.rs
use super::{Player, InternalPlayerStateUpdate, PlayerCommand, PLAYER_LOG_TARGET, command_handler}; // Use re-exported types
use std::time::Duration as StdDuration;
use tokio::time::interval;
use tracing::{error, info, trace, warn}; // Removed unused debug

/// Runs the player's command processing loop.
pub async fn run_player_loop(player: &mut Player) {
    info!(target: PLAYER_LOG_TARGET, "Player run loop started.");

    // Periodic progress reporting timer
    let mut progress_report_interval = interval(StdDuration::from_secs(5)); // Report every 5s

    loop {
        tokio::select! {
            biased; // Check commands first

            // --- Command Processing ---
            Some(command) = player.command_rx.recv() => {
                trace!(target: PLAYER_LOG_TARGET, "Received command: {:?}", command);
                match command {
                    PlayerCommand::PlayNow { item_ids, start_index } => command_handler::handle_play_now(player, item_ids, start_index).await,
                    PlayerCommand::AddToQueue { item_ids } => command_handler::handle_add_to_queue(player, item_ids).await,
                    PlayerCommand::ClearQueue => command_handler::handle_clear_queue(player).await,
                    PlayerCommand::PlayPauseToggle => command_handler::handle_play_pause_toggle(player).await,
                    // PlayerCommand::Stop removed
                    PlayerCommand::Next => command_handler::handle_next(player).await,
                    PlayerCommand::Previous => command_handler::handle_previous(player).await,
                    // PlayerCommand::SetVolume removed
                    // PlayerCommand::SetMute removed
                    PlayerCommand::GetFullState(responder) => {
                        // Directly call get_full_state from Player struct
                        let state = player.get_full_state().await;
                        let _ = responder.send(state); // Ignore error if receiver dropped
                    }
                    PlayerCommand::TrackFinished => command_handler::handle_track_finished(player).await,
                    PlayerCommand::Shutdown => {
                        info!(target: PLAYER_LOG_TARGET, "Shutdown command received. Exiting run loop.");
                        // Stop playback implicitly by breaking the loop.
                        // The audio task manager will be stopped in the final cleanup section.
                        let stopped_item_id = player.current_item_id.clone(); // Get ID before clearing state
                        // if let Some(manager) = player.audio_task_manager.take() { // REMOVED - Handled after loop
                        //      info!(target: PLAYER_LOG_TARGET, "Stopping audio task manager due to Shutdown command.");
                        //      stopped_item_id = Some(manager.item_id().to_string());
                        //      manager.stop_task().await;
                        // }
                        // Update state after stopping
                        player.is_playing = false;
                        *player.is_paused.lock().await = false;
                        player.current_item_id = None; // Clear current item ID after potential stop report

                        // Report stop if something was playing (manager existed)
                        if stopped_item_id.is_some() {
                             let _final_position = player.get_current_position().await; // Prefix unused variable
                             let snapshot = player.get_playback_state_snapshot().await; // Get snapshot after state update
                             player.reporter.report_playback_stop(&snapshot, false).await;
                        }

                        player.broadcast_update(InternalPlayerStateUpdate::Stopped); // Broadcast internal stop
                        break; // Exit the loop
                    }
                }
            }

             // --- Handle Audio Task Completion ---
             // Poll the JoinHandle from the AudioTaskManager if it exists
             // --- Handle Audio Task Completion ---
             // Correctly handle polling the JoinHandle within select!
             // We need to check if the manager exists *before* trying to poll its handle.
             // The `if` guard applies to the *branch*, not the future polling directly.
             res = async { player.audio_task_manager.as_mut().unwrap().handle().await }, if player.audio_task_manager.is_some() => {
                 // Task finished, remove the manager instance.
                 // We know it exists because of the `if` guard.
                 let finished_manager = player.audio_task_manager.take().unwrap();
                 info!(target: PLAYER_LOG_TARGET, item_id = %finished_manager.item_id(), "Audio task finished polling (completed or panicked)."); // Use getter

                 // Check if the task panicked
                 if let Err(e) = res {
                      error!(target: PLAYER_LOG_TARGET, item_id = %finished_manager.item_id(), "Audio task panicked: {:?}", e); // Use getter
                 } else {
                      trace!(target: PLAYER_LOG_TARGET, item_id = %finished_manager.item_id(), "Audio task completed polling gracefully."); // Use getter
                 }

                 // Regardless of panic or normal completion, if the task finishes *here*,
                 // it means it wasn't due to a natural end-of-track (which sends TrackFinished)
                 // or an explicit stop command. Treat it as an unexpected stop.
                 if player.is_playing { // Only report stop if we thought we were playing
                     warn!(target: PLAYER_LOG_TARGET, item_id = %finished_manager.item_id(), "Audio task stopped unexpectedly (detected via JoinHandle). Reporting stop and clearing state."); // Use getter
                     // let stopped_item_id = player.current_item_id.clone(); // Already have item_id in finished_manager
                     let _final_position = player.get_current_position().await; // Prefix unused variable
                     player.is_playing = false;
                     *player.is_paused.lock().await = false;
                     player.current_item_id = None;
                     // Report incomplete stop using the state snapshot before clearing
                     let snapshot = player.get_playback_state_snapshot().await; // Get snapshot before state is fully cleared
                     player.reporter.report_playback_stop(&snapshot, false).await; // Use reporter
                     player.broadcast_update(InternalPlayerStateUpdate::Stopped);
                 } else {
                     trace!(target: PLAYER_LOG_TARGET, item_id = %finished_manager.item_id(), "Audio task finished polling while player was already stopped/paused. No state change needed."); // Use getter
                 }
             }

            // --- Periodic Progress Reporting ---
             _ = progress_report_interval.tick(), if player.is_playing && !*player.is_paused.lock().await => {
                trace!(target: PLAYER_LOG_TARGET, "Progress report interval ticked.");
                // Report progress via HTTP POST
                let snapshot = player.get_playback_state_snapshot().await; // Call snapshot method on player
                player.reporter.report_playback_progress(&snapshot).await; // Use reporter

                // Also broadcast internal progress update
                if let Some(id) = player.current_item_id.clone() {
                     player.broadcast_update(InternalPlayerStateUpdate::Progress {
                         item_id: id,
                         position_ticks: player.get_current_position().await, // Call position method on player
                     });
                }
            }

            else => {
                // All channels closed or error occurred, break the loop
                info!(target: PLAYER_LOG_TARGET, "Command channel closed or select! error. Exiting run loop.");
                break;
            }
        }
    }

    info!(target: PLAYER_LOG_TARGET, "Player run loop finished. Performing final cleanup.");
    // 1. Ensure any active audio task is stopped
    if let Some(manager) = player.audio_task_manager.take() {
         info!(target: PLAYER_LOG_TARGET, "Stopping active audio task manager during final cleanup.");
         manager.stop_task().await; // Await task completion
    }
    // 2. Explicitly shut down the shared audio backend
    info!(target: PLAYER_LOG_TARGET, "Shutting down shared audio backend...");
    match player.audio_backend.lock().await.shutdown().await {
        Ok(_) => info!(target: PLAYER_LOG_TARGET, "Shared audio backend shutdown successful."),
        Err(e) => error!(target: PLAYER_LOG_TARGET, "Error shutting down shared audio backend: {}", e),
    }
    info!(target: PLAYER_LOG_TARGET, "Player task cleanup complete.");
}