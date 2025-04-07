use crate::jellyfin::JellyfinApiContract;
use crate::audio::SharedProgress;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex as TokioMutex, broadcast};
use tokio::task::JoinHandle; // Added for JoinHandle
use crate::audio::{PlaybackOrchestrator, PlaybackProgressInfo};

use crate::jellyfin::models::MediaItem;
// Removed unused reporter body imports
use crate::audio::playback::AudioPlaybackControl;
use tracing::{debug, info, trace, error, warn}; // Added error and warn macros
use tracing::instrument;
use crate::jellyfin::reporter::{report_playback_progress, report_playback_stopped, PlaybackProgressInfoBody, PlaybackStopInfoBody};
use tokio::time::{Duration, interval};
// use tracing::warn; // warn is already imported via the line above now

mod command_handler;
mod item_fetcher;
mod audio_task_manager;
mod playback_starter;
pub mod run_loop; // Made public
mod state;

pub use state::{PlayerCommand, InternalPlayerState, InternalPlayerStateUpdate};


/// Commands sent from the main player loop to the dedicated reporting task.
#[derive(Debug)]
pub enum ReportingCommand {
    /// Indicates playback has started or state significantly changed (pause, volume, mute).
    /// The reporting task uses this to update its internal view and potentially send progress.
    StateUpdate(InternalPlayerState), // Send the whole state for simplicity
    /// Explicitly request a progress report now.
    ReportProgressNow,
    /// Tell the reporting task to send the final 'stopped' report and terminate.
    StopAndReport,
}
/// Manages playback state, queue, and interaction with the audio backend.
pub struct Player {
    jellyfin_client: Arc<dyn JellyfinApiContract>,

    queue: Vec<MediaItem>,
    current_queue_index: usize,
    current_item_id: Option<String>,
    is_playing: bool,
    is_paused: Arc<TokioMutex<bool>>,
    is_muted: bool, // Added for reporting state
    volume_level: u32, // Added for reporting state (0-100)

    command_rx: mpsc::Receiver<PlayerCommand>,
    state_update_tx: broadcast::Sender<InternalPlayerStateUpdate>,
    internal_command_tx: mpsc::Sender<PlayerCommand>,

    audio_backend: Arc<TokioMutex<PlaybackOrchestrator>>,
    current_progress: SharedProgress,
    audio_task_manager: Option<audio_task_manager::AudioTaskManager>,
    // reporter: JellyfinReporter, // Keep reporter instance if it holds client/session
    reporting_task_handle: Option<JoinHandle<()>>, // Handle for the reporting task
    reporting_command_tx: Option<mpsc::Sender<ReportingCommand>>, // Channel to send commands to the reporting task
    is_handling_track_finish: bool, // Flag to prevent concurrent track finish handling
}

const PLAYER_LOG_TARGET: &str = "r_jellycli::player";

impl Player {
    /// Creates a new Player instance and the command channel sender.
    /// The Player itself should be run in a separate task using `Player::run`.
    pub fn new(
        jellyfin_client: Arc<dyn JellyfinApiContract>,
        alsa_device_name: String,
        state_update_capacity: usize,
        command_buffer_size: usize,
    ) -> (Self, mpsc::Sender<PlayerCommand>) {
        let (command_tx, command_rx) = mpsc::channel(command_buffer_size);
        let (state_update_tx, _) = broadcast::channel(state_update_capacity);

        let is_paused_arc = Arc::new(TokioMutex::new(false));
        let current_progress_arc = Arc::new(TokioMutex::new(PlaybackProgressInfo::default()));

        info!(target: PLAYER_LOG_TARGET, "Creating shared PlaybackOrchestrator for device: {}", alsa_device_name);
        let mut backend_instance = PlaybackOrchestrator::new(&alsa_device_name);
        backend_instance.set_pause_state_tracker(is_paused_arc.clone());
        backend_instance.set_progress_tracker(current_progress_arc.clone());

        let shared_audio_backend = Arc::new(TokioMutex::new(backend_instance));

        let player = Player {
            jellyfin_client: jellyfin_client.clone(),
            queue: Vec::new(),
            current_queue_index: 0,
            current_item_id: None,
            is_playing: false,
            is_paused: is_paused_arc,
            is_muted: false, // Default mute state
            volume_level: 100, // Default volume level
            command_rx,
            state_update_tx: state_update_tx.clone(),
            internal_command_tx: command_tx.clone(),
            audio_backend: shared_audio_backend,
            current_progress: current_progress_arc,
            audio_task_manager: None,
            // reporter: JellyfinReporter::new(jellyfin_client.clone()), // Keep if needed
            reporting_task_handle: None, // Initialize as None
            reporting_command_tx: None, // Initialize as None
            is_handling_track_finish: false,
        };

        (player, command_tx)
    }

    /// Subscribes to player state updates.
    pub fn subscribe_state_updates(&self) -> broadcast::Receiver<InternalPlayerStateUpdate> {
        self.state_update_tx.subscribe()
    }


    /// Sends a state update via the broadcast channel, logging errors.
    fn broadcast_update(&self, update: InternalPlayerStateUpdate) {
        trace!(target: PLAYER_LOG_TARGET, "Broadcasting state update: {:?}", update);
        if self.state_update_tx.send(update.clone()).is_err() {
            debug!(target: PLAYER_LOG_TARGET, "No active listeners for state update: {:?}", update);
        }
    }




    /// Gets the current playback position from the backend or returns 0.
    async fn get_current_position(&self) -> i64 {
        let progress = self.current_progress.lock().await;
        (progress.current_seconds * 10_000_000.0) as i64
    }

    /// Constructs the full current state object.
    async fn get_full_state(&self) -> InternalPlayerState {
        let current_item = self.current_item_id.as_ref()
            .and_then(|id| self.queue.iter().find(|item| &item.id == id))
            .cloned();

        InternalPlayerState {
            is_playing: self.is_playing,
            is_paused: *self.is_paused.lock().await,
            is_muted: self.is_muted, // Include new field
            volume_level: self.volume_level, // Include new field
            current_item,
            position_ticks: self.get_current_position().await,
            queue_ids: self.queue.iter().map(|item| item.id.clone()).collect(),
            current_queue_index: self.current_queue_index,
        }
    }

    // Removed unused get_playback_state_snapshot method





    /// Runs the player's command processing loop. This should be spawned as a Tokio task.
    #[instrument(skip(self))]
    pub async fn run(&mut self) {
        run_loop::run_player_loop(self).await;
    }


}


const REPORTING_LOG_TARGET: &str = "r_jellycli::player::reporting_task";
const REPORTING_INTERVAL: Duration = Duration::from_secs(10);

pub async fn run_reporting_task(
    api: Arc<dyn JellyfinApiContract>,
    initial_state: InternalPlayerState,
    mut command_rx: mpsc::Receiver<ReportingCommand>,
    current_progress: SharedProgress, // Add SharedProgress Arc
) {
    info!(target: REPORTING_LOG_TARGET, item_id = ?initial_state.current_item.as_ref().map(|i| i.id.clone()), "Reporting task started.");
    const INITIAL_POLL_INTERVAL: Duration = Duration::from_secs(1); // Poll quickly at first
    let mut report_timer = interval(INITIAL_POLL_INTERVAL); // Start with the short interval
    let mut current_state = initial_state;
    let mut initial_report_sent = false; // Re-introduce flag: true after first non-zero report
    let mut last_reported_ticks: Option<i64> = None; // Track last reported ticks
    loop {
        tokio::select! {
            biased;

            Some(command) = command_rx.recv() => {
                trace!(target: REPORTING_LOG_TARGET, "Received command: {:?}", command);
                match command {
                    ReportingCommand::StateUpdate(new_state) => {
                        trace!(target: REPORTING_LOG_TARGET, item_id = ?new_state.current_item.as_ref().map(|i| i.id.clone()), "Received state update.");
                        // Always update the full state from the snapshot provided by the main player loop.
                        // The snapshot is created using the latest available position at the time of the event.
                        current_state = new_state;
                        if let Some(_item) = &current_state.current_item {
                            // State updates (pause/volume) are logged but don't trigger reports here.
                            // The timer tick below handles detecting the initial non-zero position.
                            trace!(target: REPORTING_LOG_TARGET, item_id = ?current_state.current_item.as_ref().map(|i|&i.id), "StateUpdate received (logging only).");
                        } else {
                             warn!(target: REPORTING_LOG_TARGET, "StateUpdate received but no current item to report for.");
                        }
                    }
                    ReportingCommand::ReportProgressNow => {
                        if let Some(item) = &current_state.current_item {
                            // ReportProgressNow should force a report if playing and position changed
                            if current_state.is_playing && !current_state.is_paused {
                                let live_position_ticks = current_progress.lock().await.get_position_ticks();
                                if Some(live_position_ticks) != last_reported_ticks {
                                    trace!(target: REPORTING_LOG_TARGET, item_id = %item.id, "Explicit progress report requested ({} ticks).", live_position_ticks);
                                    let mut progress_info = build_progress_info(&current_state, "timeupdate");
                                    progress_info.position_ticks = live_position_ticks;
                                    if let Err(e) = report_playback_progress(api.as_ref(), &progress_info).await {
                                        warn!(target: REPORTING_LOG_TARGET, item_id = %item.id, "Failed to report progress (ReportProgressNow): {}", e);
                                    }
                                    last_reported_ticks = Some(live_position_ticks);
                                    // Don't reset the main timer on explicit requests
                                } else {
                                    trace!(target: REPORTING_LOG_TARGET, item_id = %item.id, "Explicit progress report requested but position unchanged. Skipping report.");
                                }
                            } else {
                                trace!(target: REPORTING_LOG_TARGET, item_id = %item.id, "Explicit progress report requested but player not playing/paused. Skipping report.");
                            }
                        } else {
                            warn!(target: REPORTING_LOG_TARGET, "ReportProgressNow received but no current item.");
                        }
                    }
                    ReportingCommand::StopAndReport => {
                        if let Some(item) = &current_state.current_item {
                            info!(target: REPORTING_LOG_TARGET, item_id = %item.id, "Stop command received. Reporting final state.");
                            let stop_info = PlaybackStopInfoBody {
                                item_id: item.id.clone(), // Use current_state's item
                                queue_item_id: None, // TODO: Get queue item ID if applicable
                                // Read live progress for final report
                                position_ticks: current_progress.lock().await.get_position_ticks(),
                            };
                            if let Err(e) = report_playback_stopped(api.as_ref(), &stop_info).await {
                                // Maybe retry once? For now, just log error.
                                error!(target: REPORTING_LOG_TARGET, item_id = %item.id, "Failed to report playback stopped: {}", e);
                            }
                        } else {
                            warn!(target: REPORTING_LOG_TARGET, "StopAndReport received but no current item was known.");
                        }
                        info!(target: REPORTING_LOG_TARGET, "Reporting task stopping.");
                        return;
                    }
                }
            }

            _ = report_timer.tick() => {
                if let Some(item) = &current_state.current_item {
                    if current_state.is_playing && !current_state.is_paused {
                        let live_position_ticks = current_progress.lock().await.get_position_ticks();

                        if !initial_report_sent {
                            // Still using the initial short poll interval
                            if live_position_ticks > 0 {
                                // First non-zero position detected!
                                info!(target: REPORTING_LOG_TARGET, item_id = %item.id, "First non-zero position detected via Timer ({} ticks). Switching to {}s interval.", live_position_ticks, REPORTING_INTERVAL.as_secs());
                                // The initial 0-tick report was sent by playback_starter.
                                // This timer tick only serves to detect the first movement and switch the interval.
                                initial_report_sent = true; // Mark that we've passed the initial phase.
                                // last_reported_ticks will be updated below, even though periodic reports are off.
                                // Switch to the standard (longer) polling interval.
                                report_timer = interval(REPORTING_INTERVAL);
                                // We need to tick immediately after resetting to maintain the new interval timing
                                report_timer.tick().await;
                            } else {
                                // Still at 0, continue polling quickly
                                trace!(target: REPORTING_LOG_TARGET, item_id = %item.id, "Initial poll timer ticked, position still 0. Continuing 1s poll.");
                            }
                        } else {
                            // Initial report already sent, now using 10s interval
                            if Some(live_position_ticks) != last_reported_ticks {
                                trace!(target: REPORTING_LOG_TARGET, item_id = %item.id, "Standard timer ticked. Reporting progress ({} ticks).", live_position_ticks);
                                // --- IMPORTANT: Periodic Progress Reporting Disabled ---
                                // Jellyfin server/UI appears to handle progress updates internally based on
                                // the initial PlaybackStart report and track duration. Sending frequent
                                // 'timeupdate' events from the client conflicts with this, causing inaccurate
                                // progress display in the UI. Disabling these periodic reports resolves the issue.
                                // The client still reports PlaybackStart, PlaybackStop, and potentially
                                // progress updates triggered by pause/seek events if implemented later.
                                // ---------------------------------------------------------
                                // let mut progress_info = build_progress_info(&current_state, "timeupdate");
                                // progress_info.position_ticks = live_position_ticks;
                                // if let Err(e) = report_playback_progress(api.as_ref(), &progress_info).await {
                                //     warn!(target: REPORTING_LOG_TARGET, item_id = %item.id, "Failed to report progress (Timer): {}", e);
                                // }
                                // last_reported_ticks = Some(live_position_ticks);
                                trace!(target: REPORTING_LOG_TARGET, item_id = %item.id, "Periodic reporting disabled. Ticks would be: {}", live_position_ticks);
                                // Still update last_reported_ticks to prevent spamming this trace log if position doesn't change
                                last_reported_ticks = Some(live_position_ticks);
                                trace!(target: REPORTING_LOG_TARGET, item_id = %item.id, "Periodic reporting disabled. Ticks would be: {}", live_position_ticks);
                                // Still update last_reported_ticks to prevent spamming this trace log if position doesn't change
                                last_reported_ticks = Some(live_position_ticks);

                            } else {
                                trace!(target: REPORTING_LOG_TARGET, item_id = %item.id, "Standard timer ticked, but position unchanged. Skipping report.");
                            }
                        }
                    } else {
                        trace!(target: REPORTING_LOG_TARGET, item_id = %item.id, "Timer ticked, but player paused/stopped. Skipping report.");
                    }
                }
                // else: No current item, nothing to report on timer tick.
            }

            else => {
                info!(target: REPORTING_LOG_TARGET, "Command channel closed. Reporting task stopping.");
                 if let Some(item) = &current_state.current_item {
                     warn!(target: REPORTING_LOG_TARGET, item_id=%item.id, "Command channel closed unexpectedly. Attempting final stop report.");
                     let stop_info = PlaybackStopInfoBody {
                         item_id: item.id.clone(),
                         queue_item_id: None,
                         // Read live progress for final report
                         position_ticks: current_progress.lock().await.get_position_ticks(),
                     };
                     if let Err(e) = report_playback_stopped(api.as_ref(), &stop_info).await {
                         error!(target: REPORTING_LOG_TARGET, item_id = %item.id, "Failed to report playback stopped (channel closed): {}", e);
                     }
                 }
                return;
            }
        }
    }
}

fn build_progress_info(state: &InternalPlayerState, event_name: &str) -> PlaybackProgressInfoBody {
    PlaybackProgressInfoBody {
        item_id: state.current_item.as_ref().map(|i| i.id.clone()).unwrap_or_default(),
        queue_item_id: None,
        can_seek: true,
        is_muted: state.is_muted,
        is_paused: state.is_paused,
        volume_level: state.volume_level,
        position_ticks: state.position_ticks,
        event_name: event_name.to_string(),
    }
}

pub async fn stop_and_clear_reporting_task(player: &mut Player) {
    if let Some(sender) = player.reporting_command_tx.take() {
        info!(target: PLAYER_LOG_TARGET, "Sending StopAndReport command to reporting task.");
        if let Err(e) = sender.send(ReportingCommand::StopAndReport).await {
            error!(target: PLAYER_LOG_TARGET, "Failed to send StopAndReport command to reporting task: {}", e);
        }
    } else {
        trace!(target: PLAYER_LOG_TARGET, "No active reporting task sender to send StopAndReport to.");
    }

    if let Some(handle) = player.reporting_task_handle.take() {
        info!(target: PLAYER_LOG_TARGET, "Aborting reporting task handle.");
        handle.abort();
    } else {
         trace!(target: PLAYER_LOG_TARGET, "No active reporting task handle to abort.");
    }
}
