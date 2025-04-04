// Removed log::trace import

use crate::jellyfin::JellyfinApiContract;
use crate::audio::SharedProgress;
use std::sync::Arc; // Removed Mutex import from std::sync
// use std::time::Duration as StdDuration; // Unused
use tokio::sync::{mpsc, Mutex as TokioMutex, broadcast}; // Use TokioMutex alias
// use tokio::task::JoinHandle; // Unused
use crate::audio::{PlaybackOrchestrator, PlaybackProgressInfo}; // Renamed AlsaPlayer
// Removed unused import: use crate::audio::AudioError;

// use crate::jellyfin::api::JellyfinError; // Unused
use crate::jellyfin::models::MediaItem;
// use crate::jellyfin::{PlaybackReportBase, QueueItem, PlaybackStoppedInfoInner}; // Unused
use crate::jellyfin::reporter::{JellyfinReporter, PlaybackStateSnapshot}; // Import new reporter types
use crate::audio::playback::AudioPlaybackControl; // Import the trait
// use tokio::sync::oneshot; // Unused
// Removed incorrect PlayerStateUpdate import from jellyfin::websocket
use tracing::{debug, info, trace}; // Removed unused error, warn
use tracing::instrument;

mod command_handler; // Declare the command handler module
mod item_fetcher; // Declare the item fetcher module
mod audio_task_manager; // Declare the audio task manager module
mod playback_starter; // Declare the playback starter module
mod run_loop; // Declare the run loop module
mod state; // Declare the state module

// Re-export key types for convenience
pub use state::{PlayerCommand, InternalPlayerState, InternalPlayerStateUpdate};

// ---------------------------------
/// Manages playback state, queue, and interaction with the audio backend.
pub struct Player {
    // --- Configuration ---
    jellyfin_client: Arc<dyn JellyfinApiContract>, // Use Arc<dyn Trait>
    alsa_device_name: String,        // Device to use for audio backend

    // --- State ---
    queue: Vec<MediaItem>,
    current_queue_index: usize,
    current_item_id: Option<String>, // ID of the currently loaded/playing item
    // position_ticks: i64, // Position is now primarily tracked by the audio backend via progress updates
    is_playing: bool, // Indicates if playback is active (not stopped)
    is_paused: Arc<TokioMutex<bool>>, // Shared pause state for audio backend loop
    // volume removed
    // is_muted removed
    // Add shuffle/repeat state later

    // --- Communication ---
    command_rx: mpsc::Receiver<PlayerCommand>, // Receives commands
    state_update_tx: broadcast::Sender<InternalPlayerStateUpdate>, // Broadcasts state changes
    // Sender for internal commands (like TrackFinished)
    internal_command_tx: mpsc::Sender<PlayerCommand>,

    // --- Audio Backend ---
    // Holds the active audio backend instance (created on play, destroyed on stop)
    _audio_backend: Option<Box<dyn AudioPlaybackControl>>, // Prefixed unused field
    // Shared progress state passed to the audio backend
    current_progress: SharedProgress,
    // Manages the currently running audio playback task
    audio_task_manager: Option<audio_task_manager::AudioTaskManager>,
    reporter: JellyfinReporter, // Add the reporter field
}

const PLAYER_LOG_TARGET: &str = "r_jellycli::player";

impl Player {
    /// Creates a new Player instance and the command channel sender.
    /// The Player itself should be run in a separate task using `Player::run`.
    pub fn new(
        jellyfin_client: Arc<dyn JellyfinApiContract>, // Use Arc<dyn Trait>
        alsa_device_name: String,
        state_update_capacity: usize, // Capacity for the state broadcast channel
        command_buffer_size: usize,   // Capacity for the command mpsc channel
    ) -> (Self, mpsc::Sender<PlayerCommand>) {
        let (command_tx, command_rx) = mpsc::channel(command_buffer_size);
        let (state_update_tx, _) = broadcast::channel(state_update_capacity);

        let player = Player {
            jellyfin_client: jellyfin_client.clone(), // Clone Arc for struct field
            alsa_device_name,
            queue: Vec::new(),
            current_queue_index: 0,
            current_item_id: None,
            is_playing: false,
            is_paused: Arc::new(TokioMutex::new(false)),
            // volume: 100, // Default volume removed
            // is_muted: false, // removed
            command_rx,
            state_update_tx: state_update_tx.clone(), // Clone sender for internal use
            internal_command_tx: command_tx.clone(), // Clone sender for internal commands
            _audio_backend: None, // Match field name with underscore prefix
            current_progress: Arc::new(TokioMutex::new(PlaybackProgressInfo::default())),
            audio_task_manager: None, // Initialize the new manager field
            reporter: JellyfinReporter::new(jellyfin_client.clone()), // Initialize reporter
        };

        (player, command_tx)
    }

    /// Subscribes to player state updates.
    pub fn subscribe_state_updates(&self) -> broadcast::Receiver<InternalPlayerStateUpdate> {
        self.state_update_tx.subscribe()
    }

    // --- Private Helper Methods ---

    /// Sends a state update via the broadcast channel, logging errors.
    fn broadcast_update(&self, update: InternalPlayerStateUpdate) {
        trace!(target: PLAYER_LOG_TARGET, "Broadcasting state update: {:?}", update);
        if self.state_update_tx.send(update.clone()).is_err() {
            // Error occurs if there are no active receivers. This is normal if nothing
            // is listening (e.g., WS sender not connected/active yet).
            debug!(target: PLAYER_LOG_TARGET, "No active listeners for state update: {:?}", update);
        }
    }

    /// Creates and initializes a new audio backend instance.
    fn create_audio_backend(&self) -> Box<dyn AudioPlaybackControl> {
        info!(target: PLAYER_LOG_TARGET, "Creating new audio backend (PlaybackOrchestrator) for device: {}", self.alsa_device_name);
        let mut backend = Box::new(PlaybackOrchestrator::new(&self.alsa_device_name));
        // Pass shared state trackers to the backend
        backend.set_pause_state_tracker(self.is_paused.clone());
        backend.set_progress_tracker(self.current_progress.clone());
        backend
    }

    // stop_audio_backend logic moved to AudioTaskManager::stop_task


    /// Gets the current playback position from the backend or returns 0.
    async fn get_current_position(&self) -> i64 {
        // Position is now tracked via the shared current_progress Arc
        let progress = self.current_progress.lock().await;
        (progress.current_seconds * 10_000_000.0) as i64
    }

    /// Constructs the full current state object.
    async fn get_full_state(&self) -> InternalPlayerState {
        let current_item = self.current_item_id.as_ref()
            .and_then(|id| self.queue.iter().find(|item| &item.id == id)) // Find item by ID
            .cloned(); // Clone the item if found

        InternalPlayerState {
            is_playing: self.is_playing,
            is_paused: *self.is_paused.lock().await,
            current_item,
            position_ticks: self.get_current_position().await,
            // volume: self.volume, // removed
            // is_muted: self.is_muted, // removed
            queue_ids: self.queue.iter().map(|item| item.id.clone()).collect(),
            current_queue_index: self.current_queue_index,
        }
    }

    /// Constructs a snapshot of the current playback state for reporting.
    async fn get_playback_state_snapshot(&self) -> PlaybackStateSnapshot {
        PlaybackStateSnapshot {
            queue: self.queue.clone(), // Clone the queue
            current_queue_index: self.current_queue_index,
            current_item_id: self.current_item_id.clone(),
            position_ticks: self.get_current_position().await,
            is_paused: *self.is_paused.lock().await,
            // Add other fields if needed (volume, mute, etc.)
        }
    }

    // fetch_and_sort_items moved to item_fetcher module

    // Reporting methods moved to JellyfinReporter

    // Command handler methods moved to command_handler module

    // --- Main Run Loop ---

    /// Runs the player's command processing loop. This should be spawned as a Tokio task.
    #[instrument(skip(self))]
    pub async fn run(&mut self) {
        run_loop::run_player_loop(self).await;
    }

    // --- Old methods removed ---

} // End impl Player

// Static helper removed
