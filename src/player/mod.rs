
use crate::jellyfin::JellyfinApiContract;
use crate::audio::SharedProgress;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex as TokioMutex, broadcast};
use crate::audio::{PlaybackOrchestrator, PlaybackProgressInfo};

use crate::jellyfin::models::MediaItem;
use crate::jellyfin::reporter::{JellyfinReporter, PlaybackStateSnapshot};
use crate::audio::playback::AudioPlaybackControl;
use tracing::{debug, info, trace};
use tracing::instrument;

mod command_handler;
mod item_fetcher;
mod audio_task_manager;
mod playback_starter;
mod run_loop;
mod state;

pub use state::{PlayerCommand, InternalPlayerState, InternalPlayerStateUpdate};

/// Manages playback state, queue, and interaction with the audio backend.
pub struct Player {
    jellyfin_client: Arc<dyn JellyfinApiContract>,

    queue: Vec<MediaItem>,
    current_queue_index: usize,
    current_item_id: Option<String>,
    is_playing: bool,
    is_paused: Arc<TokioMutex<bool>>,

    command_rx: mpsc::Receiver<PlayerCommand>,
    state_update_tx: broadcast::Sender<InternalPlayerStateUpdate>,
    internal_command_tx: mpsc::Sender<PlayerCommand>,

    audio_backend: Arc<TokioMutex<PlaybackOrchestrator>>,
    current_progress: SharedProgress,
    audio_task_manager: Option<audio_task_manager::AudioTaskManager>,
    reporter: JellyfinReporter,
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
            command_rx,
            state_update_tx: state_update_tx.clone(),
            internal_command_tx: command_tx.clone(),
            audio_backend: shared_audio_backend,
            current_progress: current_progress_arc,
            audio_task_manager: None,
            reporter: JellyfinReporter::new(jellyfin_client.clone()),
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
            current_item,
            position_ticks: self.get_current_position().await,
            queue_ids: self.queue.iter().map(|item| item.id.clone()).collect(),
            current_queue_index: self.current_queue_index,
        }
    }

    /// Constructs a snapshot of the current playback state for reporting.
    async fn get_playback_state_snapshot(&self) -> PlaybackStateSnapshot {
        PlaybackStateSnapshot {
            queue: self.queue.clone(),
            current_queue_index: self.current_queue_index,
            current_item_id: self.current_item_id.clone(),
            position_ticks: self.get_current_position().await,
            is_paused: *self.is_paused.lock().await,
        }
    }





    /// Runs the player's command processing loop. This should be spawned as a Tokio task.
    #[instrument(skip(self))]
    pub async fn run(&mut self) {
        run_loop::run_player_loop(self).await;
    }


}

