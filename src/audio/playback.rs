// src/audio/playback.rs
use std::sync::Mutex; // Use std Mutex for AlsaPcmHandler - Keep this one
// Remove potential duplicate tokio::sync::Mutex import if it exists (compiler warning indicated it)
use symphonia::core::audio::Signal;
use crate::audio::{
    alsa_handler::AlsaPcmHandler,
    decoder::{DecodeResult, SymphoniaDecoder},
    error::AudioError,
    // format_converter, // Removed unused import
    progress::{PlaybackProgressInfo, SharedProgress, PROGRESS_UPDATE_INTERVAL},
    stream_wrapper::ReqwestStreamWrapper,
};
use indicatif::{ProgressBar, ProgressStyle};
use log::{debug, error, info, trace, warn};
use reqwest::Client;
use std::sync::Arc;
use std::time::Instant;
use symphonia::core::audio::SignalSpec;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::units::TimeBase; // Removed unused Time
use tokio::sync::broadcast;
use tokio::task;

// Import the specific sample type we'll decode into for now
use symphonia::core::sample::Sample;
use std::any::TypeId; // For checking generic type S


const LOG_TARGET: &str = "r_jellycli::audio::playback"; // Main orchestrator log target

/// Manages ALSA audio playback orchestration, using dedicated handlers for ALSA, decoding, etc.
pub struct PlaybackOrchestrator { // Renamed from AlsaPlayer
    // Wrap the handler in Arc<std::sync::Mutex>
    alsa_handler: Arc<Mutex<AlsaPcmHandler>>,
    progress_bar: Option<Arc<ProgressBar>>,
    progress_info: Option<SharedProgress>,
}

impl PlaybackOrchestrator { // Renamed from AlsaPlayer
    /// Creates a new ALSA player instance for the specified device.
    pub fn new(device_name: &str) -> Self {
        info!(target: LOG_TARGET, "Creating new AlsaPlayer for device: {}", device_name);
        PlaybackOrchestrator { // Renamed from AlsaPlayer
            // Wrap the handler in Arc<std::sync::Mutex>
            alsa_handler: Arc::new(Mutex::new(AlsaPcmHandler::new(device_name))),
            progress_bar: None,
            progress_info: None,
        }
    }

    /// Sets the shared progress tracker.
    pub fn set_progress_tracker(&mut self, tracker: SharedProgress) {
        debug!(target: LOG_TARGET, "Progress tracker configured.");
        self.progress_info = Some(tracker);
    }

    // --- Private Helper Methods ---

    /// Calculates total duration and sets up the progress bar and shared info.
    fn _setup_progress(
        &mut self,
        content_length: Option<u64>,
        track_time_base: Option<TimeBase>,
        _initial_spec: Option<SignalSpec>, // Keep for potential future use (prefixed with _ to silence warning)
        total_duration_ticks: Option<i64>,
    ) -> Result<Arc<ProgressBar>, AudioError> {
        debug!(target: LOG_TARGET, "Setting up progress bar and info...");

        let total_seconds = total_duration_ticks
            .and_then(|ticks| {
                // Convert i64 ticks to u64, handling potential negative values
                u64::try_from(ticks).ok().and_then(|ts| {
                    track_time_base.map(|tb| {
                        let time = tb.calc_time(ts);
                        time.seconds as f64 + time.frac
                    })
                })
            });
            // Removed inaccurate bitrate estimation based on content_length

        if let Some(seconds) = total_seconds {
            debug!(target: LOG_TARGET, "Using total duration: {:.2} seconds", seconds);
        } else {
            debug!(target: LOG_TARGET, "Total duration unknown, will use byte progress if available or just spinner.");
        }

        // Update shared progress info
        if let Some(progress_mutex) = &self.progress_info {
            match progress_mutex.lock() {
                Ok(mut info) => {
                    info.total_seconds = total_seconds;
                    info.current_seconds = 0.0; // Reset current time
                    debug!(target: LOG_TARGET, "Shared progress info initialized.");
                }
                Err(e) => {
                    error!(target: LOG_TARGET, "Failed to lock progress info mutex for initialization: {}", e);
                }
            }
        }

        // Configure ProgressBar
        let pb = Arc::new(ProgressBar::new(0)); // Start with 0, set length later if known

        if let Some(seconds) = total_seconds {
             pb.set_length(seconds.ceil() as u64);
             pb.set_style(
                ProgressStyle::default_bar()
                    .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] ({eta})")
                    .map_err(|e| AudioError::InitializationError(format!("Invalid progress bar template: {}", e)))?
                    .progress_chars("#>-"),
            );
            debug!(target: LOG_TARGET, "Progress bar configured for time-based progress.");
        } else if let Some(len) = content_length {
             pb.set_length(len);
             pb.set_style(
                ProgressStyle::default_spinner()
                    .template("{spinner:.green} [{elapsed_precise}] {bytes}/{total_bytes} ({bytes_per_sec}, {msg})")
                    .map_err(|e| AudioError::InitializationError(format!("Invalid progress bar template: {}", e)))?,
            );
            pb.set_message("Streaming...");
            debug!(target: LOG_TARGET, "Progress bar configured for byte-based progress ({} bytes).", len);
        } else {
             // No length known, just use a spinner
             pb.set_style(
                ProgressStyle::default_spinner()
                     .template("{spinner:.green} [{elapsed_precise}] {msg}")
                     .map_err(|e| AudioError::InitializationError(format!("Invalid progress bar template: {}", e)))?,
             );
             pb.set_message("Streaming (unknown size)...");
             debug!(target: LOG_TARGET, "Progress bar configured for spinner (unknown size).");
        }

        self.progress_bar = Some(pb.clone());
        Ok(pb)
    }


    /// Updates the progress bar and shared progress info based on the current timestamp.
    fn _update_progress(
        &self,
        pb: &ProgressBar,
        current_ts: u64, // Timestamp comes from the packet/decoder result
        track_time_base: Option<TimeBase>,
        last_update_time: &mut Instant,
    ) {
        if let Some(tb) = track_time_base {
            let current_time = tb.calc_time(current_ts);
            let current_seconds = current_time.seconds as f64 + current_time.frac;

            // Update progress bar position only if it's time-based (length > 0 and not byte-based)
            let is_time_based = pb.length().unwrap_or(0) > 0; // Check if length is known (> 0)
            if is_time_based {
                 let max_pos = pb.length().unwrap(); // We know length > 0 here
                 pb.set_position( (current_seconds.floor() as u64).min(max_pos) );
            }
            // If byte-based, progress is updated by ReqwestStreamWrapper or similar mechanism

            // Update shared state periodically
            if last_update_time.elapsed() >= PROGRESS_UPDATE_INTERVAL {
                if let Some(progress_mutex) = &self.progress_info {
                    if let Ok(mut info) = progress_mutex.lock() {
                        info.current_seconds = current_seconds;
                        trace!(target: LOG_TARGET, "Updated shared progress: {:.2}s", current_seconds);
                    } else {
                        error!(target: LOG_TARGET, "Failed to lock progress info mutex for update.");
                    }
                }
                *last_update_time = Instant::now();
            }
        }
    }

    /// Writes the decoded S16LE buffer to ALSA, handling blocking and shutdown signals.
    async fn _write_to_alsa(
        &self,
        s16_buffer: &[i16],
        num_channels: usize,
        shutdown_rx: &mut broadcast::Receiver<()>,
    ) -> Result<(), AudioError> {
        if s16_buffer.is_empty() || num_channels == 0 {
            return Ok(());
        }

        let total_frames = s16_buffer.len() / num_channels;
        let mut offset = 0;

        while offset < total_frames {
            // Check shutdown before potentially blocking
            if shutdown_rx.try_recv().is_ok() {
                info!(target: LOG_TARGET, "Shutdown signal received during ALSA write loop.");
                return Ok(());
            }

            let frames_remaining = total_frames - offset;
            // Determine a reasonable chunk size to send to spawn_blocking
            // This avoids moving huge buffers unnecessarily if writei handles smaller chunks well.
            // Let's try sending chunks related to typical ALSA period sizes, e.g., 1024 or 4096 frames.
            let chunk_frames = frames_remaining.min(4096); // Example chunk size
            let _chunk_samples = chunk_frames * num_channels; // Prefixed unused variable
            let buffer_chunk = s16_buffer[offset * num_channels .. (offset + chunk_frames) * num_channels].to_vec(); // Copy chunk to move

            // Clone the Arc containing the std::sync::Mutex
            let handler_clone = Arc::clone(&self.alsa_handler); // Correct: Pass reference to Arc

            // Perform the blocking ALSA write in spawn_blocking
            let write_result = task::spawn_blocking(move || {
                // Lock the std::sync::Mutex synchronously inside the blocking thread
                match handler_clone.lock() {
                    Ok(handler_guard) => handler_guard.write_s16_buffer(&buffer_chunk), // Pass copied chunk
                    Err(poisoned) => {
                        error!(target: LOG_TARGET, "ALSA handler mutex poisoned: {}", poisoned);
                        Err(AudioError::InvalidState("ALSA handler mutex poisoned".to_string()))
                    }
                }
            }).await?; // Await the JoinHandle, propagate JoinError if task panics

            match write_result {
                 Ok(0) => { // Recovered underrun signaled by write_s16_buffer returning Ok(0)
                     warn!(target: LOG_TARGET, "Resuming playback after recovered underrun. Skipping rest of current decoded buffer.");
                     // We might want to break the inner loop or handle differently,
                     // but for now, let's just log and continue trying to write the rest (if any).
                     // If write_s16_buffer consistently returns 0 after recovery, this might loop.
                     // A better recovery might involve clearing the ALSA buffer or pausing briefly.
                     // For now, just advance offset past this problematic chunk.
                     offset += chunk_frames;
                 }
                 Ok(frames_written) if frames_written > 0 => {
                     // Note: write_s16_buffer returns frames written for the chunk
                     let actual_frames_written = frames_written.min(chunk_frames); // Ensure we don't exceed chunk size
                     offset += actual_frames_written;
                     trace!(target: LOG_TARGET, "Wrote {} frames to ALSA (total {}/{})", actual_frames_written, offset, total_frames);
                 }
                 Ok(_) => { // Should not happen if 0 means recovered underrun
                     trace!(target: LOG_TARGET, "ALSA write returned 0 frames unexpectedly, yielding.");
                     // Yield to allow ALSA buffer to drain if it returned 0 without error/recovery
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

    /// The main loop for decoding packets and sending them to ALSA.
    /// **Note:** Needs modification in decoder.rs to return timestamp with buffer.
    async fn playback_loop<S: Sample + std::fmt::Debug + Send + Sync + 'static>( // Add Send + Sync + 'static bounds
        &mut self,
        mut decoder: SymphoniaDecoder, // Keep as SymphoniaDecoder, ensure mutable
        pb: Arc<ProgressBar>,
        mut shutdown_rx: broadcast::Receiver<()>,
    ) -> Result<(), AudioError>
    { // Removed From<S> bounds, conversion handled manually below
        info!(target: LOG_TARGET, "Starting playback loop.");
        let mut last_progress_update_time = Instant::now();
        let track_time_base = decoder.time_base();

        'decode_loop: loop {
            // --- Decode Next Frame ---
            // Pass the generic type S to decode_next_frame
            let decode_result = decoder.decode_next_frame::<S>(&mut shutdown_rx).await;

            match decode_result {
                 // Assuming DecodeResult::Decoded now returns (AudioBuffer<S>, u64 timestamp)
                Ok(DecodeResult::Decoded((audio_buffer, current_ts))) => {
                    // --- Progress Update ---
                    self._update_progress(&pb, current_ts, track_time_base, &mut last_progress_update_time);

                    // --- Direct Sample Conversion to i16 ---
                    let spec = audio_buffer.spec();
                    let num_frames = audio_buffer.frames();
                    let num_channels = spec.channels.count();
                    let mut s16_vec = vec![0i16; num_frames * num_channels];

                    // Get channel planes (slices) from the owned buffer
                    let planes_data = audio_buffer.planes(); // Bind the planes data first
                    let channel_planes = planes_data.planes(); // Now get the slices

                    // Perform conversion based on the actual type S
                    let type_id_s = TypeId::of::<S>();

                    if type_id_s == TypeId::of::<i16>() {
                        // Direct copy for i16
                        for ch in 0..num_channels {
                            let plane_s16 = unsafe {
                                // Safety: We checked TypeId, so S must be i16.
                                // Reinterpret the plane data as i16.
                                std::slice::from_raw_parts(
                                    channel_planes[ch].as_ptr() as *const i16,
                                    channel_planes[ch].len(),
                                )
                            };
                            for frame in 0..num_frames {
                                s16_vec[frame * num_channels + ch] = plane_s16[frame];
                            }
                        }
                    } else if type_id_s == TypeId::of::<u8>() {
                        // Convert u8 to i16
                         for ch in 0..num_channels {
                             let plane_u8 = unsafe { std::slice::from_raw_parts(channel_planes[ch].as_ptr() as *const u8, channel_planes[ch].len()) };
                             for frame in 0..num_frames {
                                 s16_vec[frame * num_channels + ch] = (plane_u8[frame] as i16 - 128) * 256; // Removed unnecessary parentheses
                             }
                         }
                    } else if type_id_s == TypeId::of::<i32>() {
                         // Convert i32 to i16 (assuming S32 or S24 packed in i32)
                         // Note: This assumes S24 from decoder is AudioBuffer<i32>
                         for ch in 0..num_channels {
                             let plane_i32 = unsafe { std::slice::from_raw_parts(channel_planes[ch].as_ptr() as *const i32, channel_planes[ch].len()) };
                             for frame in 0..num_frames {
                                 // Check if this is S24 or S32 based on spec? For now, assume S32 conversion.
                                 // If S24, use (sample >> 8) as i16
                                 s16_vec[frame * num_channels + ch] = (plane_i32[frame] >> 16) as i16; // S32 -> S16
                             }
                         }
                    } else if type_id_s == TypeId::of::<f32>() {
                         // Convert f32 to i16
                         for ch in 0..num_channels {
                             let plane_f32 = unsafe { std::slice::from_raw_parts(channel_planes[ch].as_ptr() as *const f32, channel_planes[ch].len()) };
                             for frame in 0..num_frames {
                                 s16_vec[frame * num_channels + ch] = (plane_f32[frame] * 32767.0).clamp(-32768.0, 32767.0) as i16;
                             }
                         }
                    } else if type_id_s == TypeId::of::<f64>() {
                         // Convert f64 to i16
                         for ch in 0..num_channels {
                             let plane_f64 = unsafe { std::slice::from_raw_parts(channel_planes[ch].as_ptr() as *const f64, channel_planes[ch].len()) };
                             for frame in 0..num_frames {
                                 s16_vec[frame * num_channels + ch] = (plane_f64[frame] * 32767.0).clamp(-32768.0, 32767.0) as i16;
                             }
                         }
                    } else {
                         warn!(target: LOG_TARGET, "Skipping buffer: unsupported sample type {:?} for direct S16 conversion.", TypeId::of::<S>());
                         continue; // Skip this buffer
                    };

                    // --- ALSA Playback ---
                    let num_channels = audio_buffer.spec().channels.count();
                    if let Err(e) = self._write_to_alsa(&s16_vec, num_channels, &mut shutdown_rx).await {
                        pb.abandon_with_message(format!("ALSA Write Error: {}", e));
                        return Err(e);
                    }
                }
                Ok(DecodeResult::Skipped(reason)) => {
                     warn!(target: LOG_TARGET, "Decoder skipped packet: {}", reason);
                     continue;
                }
                Ok(DecodeResult::EndOfStream) => {
                    info!(target: LOG_TARGET, "Decoder reached end of stream.");
                    pb.finish_with_message("Playback finished");
                    break 'decode_loop;
                }
                 Ok(DecodeResult::Shutdown) => {
                    info!(target: LOG_TARGET, "Decoder received shutdown signal.");
                    pb.abandon_with_message("Playback stopped");
                    break 'decode_loop;
                }
                Err(e) => {
                    error!(target: LOG_TARGET, "Fatal decoder error: {}", e);
                    pb.abandon_with_message(format!("Decoder Error: {}", e));
                    return Err(e);
                }
            }
        } // end 'decode_loop

        // --- Post-Loop Cleanup ---
        info!(target: LOG_TARGET, "Playback loop finished.");
        // Lock the mutex before calling drain
        if let Ok(guard) = self.alsa_handler.lock() { // Removed mut
            if let Err(e) = guard.drain() {
                error!(target: LOG_TARGET, "Error draining ALSA device: {}", e);
                // Moved misplaced warn! inside the relevant error block
                warn!(target: LOG_TARGET, "Error draining ALSA buffer after EOF: {}", e);
            }
        } else {
            error!(target: LOG_TARGET, "Failed to lock ALSA handler mutex during drain.");
        }
        // Removed extra closing brace from line 383

        Ok(())
    }

    // --- Public Methods ---

    /// Streams audio from a URL, decodes it, plays it via ALSA, and updates progress.
    pub async fn stream_decode_and_play(
        &mut self,
        url: &str,
        total_duration_ticks: Option<i64>,
        shutdown_rx: broadcast::Receiver<()>,
    ) -> Result<(), AudioError> {
        info!(target: LOG_TARGET, "Starting stream/decode/play for URL: {}", url);

        // --- HTTP Streaming Setup ---
        let client = Client::new();
        let response = client.get(url).send().await?.error_for_status()?;
        let content_length = response.content_length();
        debug!(target: LOG_TARGET, "HTTP response received. Content-Length: {:?}", content_length);
        let stream = response.bytes_stream();
        let source = Box::new(ReqwestStreamWrapper::new_async(stream).await?);
        let mss = MediaSourceStream::new(source, Default::default());

        // --- Symphonia Decoder Setup ---
        let decoder = SymphoniaDecoder::new(mss)?; // Removed mut
        let initial_spec = decoder.initial_spec().ok_or(AudioError::InitializationError("Decoder failed to provide initial spec".to_string()))?;
        let track_time_base = decoder.time_base();

        // --- ALSA Initialization ---
        // Lock the mutex to initialize
        self.alsa_handler.lock().map_err(|e| AudioError::InvalidState(format!("ALSA handler mutex poisoned on init: {}", e)))?.initialize(initial_spec)?; // Pass owned String

        // --- Progress Bar & Info Setup ---
        let pb = self._setup_progress(content_length, track_time_base, Some(initial_spec), total_duration_ticks)?;

        // --- Call Playback Loop ---
        // We need to call the generic playback_loop. Let's assume f32 for now.
        // A better approach might inspect the initial_spec.sample_format()
        // let loop_result = self.playback_loop::<f32>(decoder, pb, shutdown_rx).await;

        // Determine sample type from spec and call the appropriate generic loop
        // This requires more complex dispatch or macros.
        // For now, let's stick to f32 as the assumed intermediate type.
        // **This is a limitation to address later.**
         let loop_result = self.playback_loop::<f32>(decoder, pb, shutdown_rx).await;


        // --- End Playback Loop ---
        loop_result
    }


    /// Closes the audio device and cleans up resources like the progress bar.
    pub fn close(&mut self) {
        info!(target: LOG_TARGET, "Closing AlsaPlayer resources.");
        // Lock the mutex to close
        if let Ok(mut guard) = self.alsa_handler.lock() {
             guard.close();
        } else {
             error!(target: LOG_TARGET, "Failed to lock ALSA handler mutex during close.");
        }
        if let Some(pb) = self.progress_bar.take() {
            if !pb.is_finished() {
                pb.abandon_with_message("Playback stopped");
                debug!(target: LOG_TARGET, "Abandoned progress bar.");
            } else {
                debug!(target: LOG_TARGET, "Progress bar already finished.");
            }
        }
        if let Some(progress_mutex) = &self.progress_info {
            if let Ok(mut info) = progress_mutex.lock() {
                *info = PlaybackProgressInfo::default();
                debug!(target: LOG_TARGET, "Reset shared progress info.");
            } else {
                error!(target: LOG_TARGET, "Failed to lock progress info mutex during close.");
            }
        }
    }
}

impl Drop for PlaybackOrchestrator { // Renamed from AlsaPlayer
    fn drop(&mut self) {
        debug!(target: LOG_TARGET, "Dropping PlaybackOrchestrator.");
        self.close();
    }
}
