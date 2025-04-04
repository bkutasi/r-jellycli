use crate::audio::{alsa_handler::AlsaPcmHandler, error::AudioError};
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;
use tokio::task;
use tracing::{debug, error, info, trace, warn, instrument};

const LOG_TARGET: &str = "r_jellycli::audio::alsa_writer";

/// Handles asynchronous interaction with the ALSA PCM device.
/// Wraps the synchronous AlsaPcmHandler using tokio::task::spawn_blocking.
pub struct AlsaWriter {
    alsa_handler: Arc<Mutex<AlsaPcmHandler>>,
}

impl AlsaWriter {
    /// Creates a new AlsaWriter.
    pub fn new(alsa_handler: Arc<Mutex<AlsaPcmHandler>>) -> Self {
        Self { alsa_handler }
    }

    /// Asynchronously writes an S16LE buffer to the ALSA device.
    /// Handles blocking writes and shutdown signals.
    #[instrument(skip(self, s16_buffer, shutdown_rx), fields(frames = s16_buffer.len() / num_channels.max(1)))]
    pub async fn write_s16_buffer_async(
        &self,
        s16_buffer: Vec<i16>,
        num_channels: usize,
        shutdown_rx: &mut broadcast::Receiver<()>,
    ) -> Result<(), AudioError> {
        if s16_buffer.is_empty() || num_channels == 0 {
            trace!(target: LOG_TARGET, "Skipping write for empty buffer or zero channels.");
            return Ok(());
        }

        let total_frames = s16_buffer.len() / num_channels;
        let mut offset = 0;

        while offset < total_frames {
            // Check shutdown before potentially blocking
            if shutdown_rx.try_recv().is_ok() {
                info!(target: LOG_TARGET, "Shutdown signal received during ALSA write loop. Returning ShutdownRequested.");
                return Err(AudioError::ShutdownRequested);
            }

            let frames_remaining = total_frames - offset;
            // Determine a reasonable chunk size to send to spawn_blocking
            let chunk_frames = frames_remaining.min(4096); // Example chunk size
            let buffer_chunk = s16_buffer[offset * num_channels .. (offset + chunk_frames) * num_channels].to_vec();

            let handler_clone = Arc::clone(&self.alsa_handler);

            trace!(target: LOG_TARGET, "Calling alsa_handler.write_s16_buffer with {} frames in blocking task...", chunk_frames);
            let write_result = task::spawn_blocking(move || {
                match handler_clone.lock() {
                    Ok(handler_guard) => handler_guard.write_s16_buffer(&buffer_chunk),
                    Err(poisoned) => {
                        error!(target: LOG_TARGET, "ALSA handler mutex poisoned: {}", poisoned);
                        Err(AudioError::InvalidState("ALSA handler mutex poisoned".to_string()))
                    }
                }
            }).await?;

            trace!(target: LOG_TARGET, "alsa_handler.write_s16_buffer result: {:?}", write_result.as_ref().map_err(|e| format!("{:?}", e)));
            match write_result {
                 Ok(0) => { // Recovered underrun
                     warn!(target: LOG_TARGET, "ALSA underrun recovered, retrying write for the same chunk.");
                     tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                     continue; // Retry the same chunk
                 }
                 Ok(frames_written) if frames_written > 0 => {
                     let actual_frames_written = frames_written.min(chunk_frames);
                     offset += actual_frames_written;
                     trace!(target: LOG_TARGET, "Wrote {} frames to ALSA (total {}/{})", actual_frames_written, offset, total_frames);
                 }
                 Ok(_) => { // Should not happen if 0 means recovered underrun
                     trace!(target: LOG_TARGET, "ALSA write returned 0 frames unexpectedly, yielding.");
                     task::yield_now().await;
                 }
                 Err(e @ AudioError::AlsaError(_)) => {
                     error!(target: LOG_TARGET, "Unrecoverable ALSA write error: {}", e);
                     return Err(e);
                 }
                  Err(e) => {
                      error!(target: LOG_TARGET, "Unexpected error during ALSA write: {}", e);
                      return Err(e);
                  }
            }
        }
        Ok(())
    }

    /// Asynchronously pauses the ALSA device.
    #[instrument(skip(self))]
    pub async fn pause_async(&self) -> Result<(), AudioError> {
        debug!(target: LOG_TARGET, "Requesting ALSA pause.");
        let handler_clone = Arc::clone(&self.alsa_handler);
        task::spawn_blocking(move || {
            match handler_clone.lock() {
                Ok(handler_guard) => handler_guard.pause(),
                Err(poisoned) => {
                    error!(target: LOG_TARGET, "ALSA handler mutex poisoned during pause attempt: {}", poisoned);
                    Err(AudioError::InvalidState("ALSA handler mutex poisoned".to_string()))
                }
            }
        })
        .await?
        .map_err(|e| {
            warn!(target: LOG_TARGET, "Failed to pause ALSA device: {}", e);
            e
        })
    }

    /// Asynchronously resumes the ALSA device.
    #[instrument(skip(self))]
    pub async fn resume_async(&self) -> Result<(), AudioError> {
        debug!(target: LOG_TARGET, "Requesting ALSA resume.");
        let handler_clone = Arc::clone(&self.alsa_handler);
        task::spawn_blocking(move || {
             match handler_clone.lock() {
                Ok(handler_guard) => {
                    trace!(target: LOG_TARGET, "Inside blocking task: Calling handler_guard.resume()...");
                    let res = handler_guard.resume();
                    trace!(target: LOG_TARGET, "Inside blocking task: handler_guard.resume() returned: {:?}", res.as_ref().map_err(|e| format!("{:?}", e)));
                    res
                }
                Err(poisoned) => {
                    error!(target: LOG_TARGET, "ALSA handler mutex poisoned during resume attempt: {}", poisoned);
                    Err(AudioError::InvalidState("ALSA handler mutex poisoned".to_string()))
                }
            }
        })
        .await?
        .map_err(|e| {
            warn!(target: LOG_TARGET, "Failed to resume ALSA device: {}", e);
            e
        })
    }

    /// Asynchronously drains the ALSA buffer.
    #[instrument(skip(self))]
    pub async fn drain_async(&self) -> Result<(), AudioError> {
        debug!(target: LOG_TARGET, "Requesting ALSA drain.");
        let handler_clone = Arc::clone(&self.alsa_handler);
        task::spawn_blocking(move || {
            match handler_clone.lock() {
                Ok(handler_guard) => handler_guard.drain(),
                Err(poisoned) => {
                    error!(target: LOG_TARGET, "ALSA handler mutex poisoned during drain attempt: {}", poisoned);
                    Err(AudioError::InvalidState("ALSA handler mutex poisoned".to_string()))
                }
            }
        })
        .await?
        .map_err(|e| {
            error!(target: LOG_TARGET, "Error draining ALSA buffer: {}", e);
            e
        })
    }

    /// Asynchronously shuts down the ALSA device (closes PCM handle).
    #[instrument(skip(self))]
    pub async fn shutdown_async(&self) -> Result<(), AudioError> {
        info!(target: LOG_TARGET, "Shutting down ALSA handler asynchronously.");
        let handler_clone = Arc::clone(&self.alsa_handler);
        task::spawn_blocking(move || {
            match handler_clone.lock() {
                Ok(mut guard) => {
                    guard.shutdown_device(); // This internally calls close -> drop -> snd_pcm_close
                    debug!(target: LOG_TARGET, "ALSA handler shutdown_device() called successfully within blocking task.");
                    Ok(())
                }
                Err(poisoned) => {
                    error!(target: LOG_TARGET, "ALSA handler mutex poisoned during blocking shutdown: {}", poisoned);
                    Err(AudioError::InvalidState("ALSA handler mutex poisoned during shutdown".to_string()))
                }
            }
        })
        .await?
        .map_err(|e| {
            error!(target: LOG_TARGET, "ALSA handler shutdown task returned error: {}", e);
            e
        })
    }
}