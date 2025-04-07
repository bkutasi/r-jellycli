use crate::audio::{
    error::AudioError,
    progress::{PlaybackProgressInfo, SharedProgress},
};
use std::{sync::Arc, time::Instant};
use symphonia::core::units::TimeBase;
use tokio::sync::Mutex as TokioMutex;
use tracing::{debug, info, trace, warn, instrument};

const LOG_TARGET: &str = "r_jellycli::audio::state_manager";

/// Callback type for when playback finishes naturally.
pub type OnFinishCallback = Box<dyn FnOnce() + Send + Sync + 'static>;

/// Manages playback state including pause, progress, and finish callback.
pub struct PlaybackStateManager {
    progress_info: Option<SharedProgress>,
    pause_state: Option<Arc<TokioMutex<bool>>>,
    on_finish_callback: Option<OnFinishCallback>,
    last_progress_log_time: Instant, // Track last log time
}

impl PlaybackStateManager {
    /// Creates a new PlaybackStateManager.
    pub fn new() -> Self {
        Self {
            progress_info: None,
            pause_state: None,
            on_finish_callback: None,
            last_progress_log_time: Instant::now(),
        }
    }

    /// Sets the shared progress tracker.
    pub fn set_progress_tracker(&mut self, tracker: SharedProgress) {
        debug!(target: LOG_TARGET, "Progress tracker configured.");
        self.progress_info = Some(tracker);
    }

    /// Sets the shared pause state tracker.
    pub fn set_pause_state_tracker(&mut self, state: Arc<TokioMutex<bool>>) {
        debug!(target: LOG_TARGET, "Pause state tracker configured.");
        self.pause_state = Some(state);
    }

    /// Sets the callback to be executed when playback finishes naturally.
    pub fn set_on_finish_callback(&mut self, callback: OnFinishCallback) {
        info!(target: LOG_TARGET, "On finish callback configured.");
        self.on_finish_callback = Some(callback);
    }

    /// Updates the shared progress information based on the current timestamp.
    /// Returns true if the progress was updated, false otherwise (e.g., due to rate limiting).
    #[instrument(skip(self), fields(current_ts))]
    pub async fn update_progress(&mut self, current_ts: u64, track_time_base: Option<TimeBase>) {
        // Note: This function no longer returns bool as it always updates progress now.
        // The rate limiting is only for logging.
        // Removed rate limiting - update progress on every call

        if let (Some(progress_arc), Some(time_base)) = (&self.progress_info, track_time_base) {
            // Calculate current playback time in seconds using the original timestamp and time base.
            let current_seconds = time_base.calc_time(current_ts).seconds as f64
                + time_base.calc_time(current_ts).frac;

            // Log progress update roughly every second
            let should_log = self.last_progress_log_time.elapsed() >= std::time::Duration::from_secs(1);
            if should_log {
                trace!(target: LOG_TARGET, "Updating progress (throttled log): TS={}, Seconds={:.2}", current_ts, current_seconds);
                self.last_progress_log_time = Instant::now();
            }

            let mut progress_guard = progress_arc.lock().await;
            progress_guard.current_seconds = current_seconds;
        } else {
            if self.progress_info.is_none() {
                trace!(target: LOG_TARGET, "Skipping progress update: progress_info not set.");
            }
            if track_time_base.is_none() {
                trace!(target: LOG_TARGET, "Skipping progress update: track_time_base not set.");
            }
            // No return value needed now
        }
    }

    /// Gets the current playback position in ticks (based on last progress update).
    #[instrument(skip(self))]
    pub async fn get_current_position_ticks(&self) -> Result<i64, AudioError> {
        trace!(target: LOG_TARGET, "Getting current position ticks.");
        if let Some(progress_arc) = &self.progress_info {
            let progress_guard = progress_arc.lock().await;
            // Convert seconds back to ticks (assuming 1 tick = 100ns, standard for Jellyfin)
            let position_ticks = (progress_guard.current_seconds * 10_000_000.0) as i64;
            trace!(target: LOG_TARGET, "Returning position_ticks: {}", position_ticks);
            Ok(position_ticks)
        } else {
            trace!(target: LOG_TARGET, "Progress info not available, returning 0 ticks.");
            Ok(0)
        }
    }

    /// Sets the shared pause state to true.
    #[instrument(skip(self))]
    pub async fn pause(&self) -> Result<(), AudioError> {
        info!(target: LOG_TARGET, "Setting pause state to true.");
        if let Some(state_arc) = &self.pause_state {
            let mut guard = state_arc.lock().await;
            *guard = true;
            Ok(())
        } else {
            warn!(target: LOG_TARGET, "Pause called, but pause state tracker is not set.");
            Err(AudioError::InvalidState("Pause state tracker not configured".to_string()))
        }
    }

    /// Sets the shared pause state to false.
    #[instrument(skip(self))]
    pub async fn resume(&self) -> Result<(), AudioError> {
        info!(target: LOG_TARGET, "Setting pause state to false.");
        if let Some(state_arc) = &self.pause_state {
            let mut guard = state_arc.lock().await;
            *guard = false;
            Ok(())
        } else {
            warn!(target: LOG_TARGET, "Resume called, but pause state tracker is not set.");
            Err(AudioError::InvalidState("Pause state tracker not configured".to_string()))
        }
    }

    /// Checks the current value of the shared pause state.
    #[instrument(skip(self))]
    pub async fn is_paused(&self) -> bool {
        if let Some(state_arc) = &self.pause_state {
            *state_arc.lock().await
        } else {
            trace!(target: LOG_TARGET, "Pause state checked, but tracker not set. Assuming not paused.");
            false
        }
    }

    /// Takes and executes the `on_finish` callback if it exists.
    #[instrument(skip(self))]
    pub fn execute_on_finish_callback(&mut self) {
        if let Some(callback) = self.on_finish_callback.take() {
            info!(target: LOG_TARGET, "Executing on_finish callback.");
            callback();
        } else {
            warn!(target: LOG_TARGET, "Attempted to execute on_finish callback, but none was set or it was already taken.");
        }
    }

    /// Resets the shared progress information to default values.
    #[instrument(skip(self))]
    pub async fn reset_progress_info(&mut self) {
         if let Some(progress_mutex) = self.progress_info.take() {
            debug!(target: LOG_TARGET, "Resetting shared progress info...");
            let mut info = progress_mutex.lock().await;
            *info = PlaybackProgressInfo::default();
            debug!(target: LOG_TARGET, "Reset shared progress info.");
        } else {
            debug!(target: LOG_TARGET, "No progress info tracker set, skipping reset.");
        }
    }

    /// Provides access to the shared pause state Arc for the playback loop.
    pub fn get_pause_state_tracker(&self) -> Option<Arc<TokioMutex<bool>>> {
        self.pause_state.clone()
    }
}

impl Default for PlaybackStateManager {
    fn default() -> Self {
        Self::new()
    }
}