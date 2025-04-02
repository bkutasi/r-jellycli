// src/audio/playback.rs
use std::sync::Mutex; // Use std Mutex for AlsaPcmHandler - Keep this one
use tokio::sync::Mutex as TokioMutex;
// Remove potential duplicate tokio::sync::Mutex import if it exists (compiler warning indicated it)
use symphonia::core::audio::Signal;
use rubato::{Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction};
use crate::audio::{
    alsa_handler::AlsaPcmHandler,
    // Import the new types from decoder
    decoder::{DecodeRefResult, DecodedBufferAndTimestamp, SymphoniaDecoder},
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
use symphonia::core::io::MediaSourceStreamOptions;
use symphonia::core::units::TimeBase;
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
    resampler: Option<Arc<TokioMutex<SincFixedIn<f32>>>>,
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
            resampler: None,
        }
    }

    /// Sets the shared progress tracker.
    pub fn set_progress_tracker(&mut self, tracker: SharedProgress) {
        debug!(target: LOG_TARGET, "Progress tracker configured.");
        self.progress_info = Some(tracker);
    }

    // --- Private Helper Methods ---

    /// Calculates total duration and sets up the progress bar and shared info.
    async fn _setup_progress( // Changed to async fn
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
            // Lock the async mutex
            let mut info = progress_mutex.lock().await;
            info.total_seconds = total_seconds;
            info.current_seconds = 0.0; // Reset current time
            debug!(target: LOG_TARGET, "Shared progress info initialized.");
            // Note: Tokio's mutex lock doesn't return Result, it panics on poison.
            // Removed the previous error handling for PoisonError.
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
    async fn _update_progress( // Changed to async fn
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
                    // Lock the async mutex
                    let mut info = progress_mutex.lock().await;
                    info.current_seconds = current_seconds;
                    trace!(target: LOG_TARGET, "Updated shared progress: {:.2}s", current_seconds);
                    // Note: Tokio's mutex lock doesn't return Result, it panics on poison.
                    // Removed the previous error handling for PoisonError.
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
            trace!(target: LOG_TARGET, "Calling alsa_handler.write_s16_buffer with {} frames in blocking task...", chunk_frames);
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

            trace!(target: LOG_TARGET, "alsa_handler.write_s16_buffer result: {:?}", write_result.as_ref().map_err(|e| format!("{:?}", e)));
            match write_result {
                 Ok(0) => { // Recovered underrun signaled by write_s16_buffer returning Ok(0)
                     warn!(target: LOG_TARGET, "ALSA underrun recovered, retrying write for the same chunk.");
                     // Don't advance offset, retry the same chunk.
                     // Add a small sleep to avoid busy-looping if ALSA isn't ready immediately.
                     tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                     continue; // Continue the while loop to retry the chunk
                 }
                 // Removed extra closing brace that was here
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

    /// Converts a generic Symphonia AudioBuffer into an interleaved S16LE Vec.
    fn _convert_buffer_to_s16<S: Sample + 'static>(
        &self,
        audio_buffer: symphonia::core::audio::AudioBuffer<S>,
    ) -> Result<Vec<i16>, AudioError> {
        let spec = audio_buffer.spec();
        let num_frames = audio_buffer.frames();
        let num_channels = spec.channels.count();
        let mut s16_vec = vec![0i16; num_frames * num_channels];

        let type_id_s = TypeId::of::<S>();
        let planes_data = audio_buffer.planes();
        let channel_planes = planes_data.planes(); // Get the slices

        trace!(target: LOG_TARGET, "Converting buffer ({} frames, {} channels, type: {:?}) to S16LE", num_frames, num_channels, type_id_s);

        // --- Conversion Logic (adapted from old code) ---
        if type_id_s == TypeId::of::<i16>() {
            trace!(target: LOG_TARGET, "Input is S16");
            // Safety: We checked TypeId. Accessing as *const i16.
            if num_channels == 1 {
                let plane_s16 = unsafe { std::slice::from_raw_parts(channel_planes[0].as_ptr() as *const i16, num_frames) };
                s16_vec.copy_from_slice(plane_s16);
            } else {
                for frame in 0..num_frames {
                    for ch in 0..num_channels {
                        let sample_s16 = unsafe { *(channel_planes[ch].as_ptr() as *const i16).add(frame) };
                        s16_vec[frame * num_channels + ch] = sample_s16;
                    }
                }
            }
        } else if type_id_s == TypeId::of::<u8>() {
            trace!(target: LOG_TARGET, "Input is U8");
            // Safety: We checked TypeId. Accessing as *const u8.
            if num_channels == 1 {
                let plane_u8 = unsafe { std::slice::from_raw_parts(channel_planes[0].as_ptr() as *const u8, num_frames) };
                for frame in 0..num_frames {
                    s16_vec[frame] = ((plane_u8[frame] as i16 - 128) * 256) as i16;
                }
            } else {
                for frame in 0..num_frames {
                    for ch in 0..num_channels {
                        let sample_u8 = unsafe { *(channel_planes[ch].as_ptr() as *const u8).add(frame) };
                        s16_vec[frame * num_channels + ch] = ((sample_u8 as i16 - 128) * 256) as i16;
                    }
                }
            }
        } else if type_id_s == TypeId::of::<i32>() {
            trace!(target: LOG_TARGET, "Input is S32/S24");
             // Safety: We checked TypeId. Accessing as *const i32.
            // Assumes S32 or S24 packed in i32. Convert to S16 by right-shifting.
            if num_channels == 1 {
                let plane_i32 = unsafe { std::slice::from_raw_parts(channel_planes[0].as_ptr() as *const i32, num_frames) };
                for frame in 0..num_frames {
                    s16_vec[frame] = (plane_i32[frame] >> 16) as i16; // S32 -> S16
                }
            } else {
                for frame in 0..num_frames {
                    for ch in 0..num_channels {
                        let sample_i32 = unsafe { *(channel_planes[ch].as_ptr() as *const i32).add(frame) };
                        s16_vec[frame * num_channels + ch] = (sample_i32 >> 16) as i16; // S32 -> S16
                    }
                }
            }
        } else if type_id_s == TypeId::of::<f32>() {
            trace!(target: LOG_TARGET, "Input is F32");
            // Safety: We checked TypeId. Accessing as *const f32.
            if num_channels == 1 {
                let plane_f32 = unsafe { std::slice::from_raw_parts(channel_planes[0].as_ptr() as *const f32, num_frames) };
                for frame in 0..num_frames {
                    s16_vec[frame] = (plane_f32[frame] * 32767.0).clamp(-32768.0, 32767.0) as i16;
                }
            } else {
                for frame in 0..num_frames {
                    for ch in 0..num_channels {
                        let sample_f32 = unsafe { *(channel_planes[ch].as_ptr() as *const f32).add(frame) };
                        s16_vec[frame * num_channels + ch] = (sample_f32 * 32767.0).clamp(-32768.0, 32767.0) as i16;
                    }
                }
            }
        } else if type_id_s == TypeId::of::<f64>() {
             trace!(target: LOG_TARGET, "Input is F64");
             // Safety: We checked TypeId. Accessing as *const f64.
            if num_channels == 1 {
                let plane_f64 = unsafe { std::slice::from_raw_parts(channel_planes[0].as_ptr() as *const f64, num_frames) };
                for frame in 0..num_frames {
                    s16_vec[frame] = (plane_f64[frame] * 32767.0).clamp(-32768.0, 32767.0) as i16;
                }
            } else {
                for frame in 0..num_frames {
                    for ch in 0..num_channels {
                        let sample_f64 = unsafe { *(channel_planes[ch].as_ptr() as *const f64).add(frame) };
                        s16_vec[frame * num_channels + ch] = (sample_f64 * 32767.0).clamp(-32768.0, 32767.0) as i16;
                    }
                }
            }
        } else {
            warn!(target: LOG_TARGET, "Unsupported sample type {:?} for direct S16 conversion.", TypeId::of::<S>());
            // Return silence or an error? Let's return an error.
             return Err(AudioError::UnsupportedFormat("Cannot convert decoded format to S16".to_string()));
        }

        Ok(s16_vec)
    }
    /// Converts a generic Symphonia AudioBuffer into Vec<Vec<f32>> suitable for Rubato.
    fn _convert_buffer_to_f32_vecs<S: Sample + 'static>(
        &self,
        audio_buffer: symphonia::core::audio::AudioBuffer<S>,
    ) -> Result<Vec<Vec<f32>>, AudioError> {
        let spec = audio_buffer.spec();
        let num_frames = audio_buffer.frames();
        let num_channels = spec.channels.count();
        let mut f32_vecs: Vec<Vec<f32>> = vec![vec![0.0f32; num_frames]; num_channels];

        let type_id_s = TypeId::of::<S>();
        let planes_data = audio_buffer.planes();
        let channel_planes = planes_data.planes(); // Get the slices

        trace!(target: LOG_TARGET, "Converting buffer ({} frames, {} channels, type: {:?}) to Vec<Vec<f32>>", num_frames, num_channels, type_id_s);

        // --- Conversion Logic ---
        if type_id_s == TypeId::of::<i16>() {
            for ch in 0..num_channels {
                let plane_s16 = unsafe { std::slice::from_raw_parts(channel_planes[ch].as_ptr() as *const i16, num_frames) };
                for frame in 0..num_frames {
                    f32_vecs[ch][frame] = plane_s16[frame] as f32 / 32768.0;
                }
            }
        } else if type_id_s == TypeId::of::<u8>() {
            for ch in 0..num_channels {
                let plane_u8 = unsafe { std::slice::from_raw_parts(channel_planes[ch].as_ptr() as *const u8, num_frames) };
                for frame in 0..num_frames {
                    f32_vecs[ch][frame] = ((plane_u8[frame] as i16 - 128) as f32) / 128.0;
                }
            }
        } else if type_id_s == TypeId::of::<i32>() { // Handles S32 and S24
            for ch in 0..num_channels {
                let plane_i32 = unsafe { std::slice::from_raw_parts(channel_planes[ch].as_ptr() as *const i32, num_frames) };
                for frame in 0..num_frames {
                    // Assuming S24/S32 input, scale to f32 range
                    f32_vecs[ch][frame] = (plane_i32[frame] as f64 / 2147483648.0) as f32; // Normalize S32 range
                }
            }
        } else if type_id_s == TypeId::of::<f32>() {
            for ch in 0..num_channels {
                let plane_f32 = unsafe { std::slice::from_raw_parts(channel_planes[ch].as_ptr() as *const f32, num_frames) };
                f32_vecs[ch].copy_from_slice(plane_f32);
            }
        } else if type_id_s == TypeId::of::<f64>() {
             for ch in 0..num_channels {
                let plane_f64 = unsafe { std::slice::from_raw_parts(channel_planes[ch].as_ptr() as *const f64, num_frames) };
                for frame in 0..num_frames {
                    f32_vecs[ch][frame] = plane_f64[frame] as f32; // Direct conversion, assuming f64 is already in [-1.0, 1.0]
                }
            }
        } else {
            warn!(target: LOG_TARGET, "Unsupported sample type {:?} for F32 conversion.", TypeId::of::<S>());
            return Err(AudioError::UnsupportedFormat("Cannot convert decoded format to F32 for resampling".to_string()));
        }

        Ok(f32_vecs)
    }

    /// Converts Vec<Vec<f32>> (output from Rubato) into an interleaved S16LE Vec.
    fn _convert_f32_vecs_to_s16(
        &self,
        f32_vecs: Vec<Vec<f32>>,
    ) -> Result<Vec<i16>, AudioError> {
        if f32_vecs.is_empty() || f32_vecs[0].is_empty() {
            return Ok(Vec::new()); // Return empty if input is empty
        }

        let num_channels = f32_vecs.len();
        let num_frames = f32_vecs[0].len(); // Assume all channels have the same length
        let mut s16_vec = vec![0i16; num_frames * num_channels];

        trace!(target: LOG_TARGET, "Converting Vec<Vec<f32>> ({} frames, {} channels) to interleaved S16LE", num_frames, num_channels);

        for frame in 0..num_frames {
            for ch in 0..num_channels {
                // Ensure channel exists and frame index is valid
                if ch < f32_vecs.len() && frame < f32_vecs[ch].len() {
                    let sample_f32 = f32_vecs[ch][frame];
                    // Scale f32 [-1.0, 1.0] to i16 [-32768, 32767]
                    s16_vec[frame * num_channels + ch] = (sample_f32 * 32767.0).clamp(-32768.0, 32767.0) as i16;
                } else {
                    // Handle potential inconsistency in channel lengths (shouldn't happen with rubato)
                    warn!(target: LOG_TARGET, "Inconsistent channel lengths detected during F32 to S16 conversion at frame {}, channel {}", frame, ch);
                    s16_vec[frame * num_channels + ch] = 0; // Fill with silence
                }
            }
        }

        Ok(s16_vec)
    }


    /// The main loop for decoding packets and sending them to ALSA.
    async fn playback_loop( // Removed generic type <S>
        &mut self,
        mut decoder: SymphoniaDecoder,
        pb: Arc<ProgressBar>,
        mut shutdown_rx: broadcast::Receiver<()>,
    ) -> Result<(), AudioError> {
        info!(target: LOG_TARGET, "Starting playback loop.");
        let mut last_progress_update_time = Instant::now();
        let track_time_base = decoder.time_base();

        'decode_loop: loop {
            trace!(target: LOG_TARGET, "--- Playback loop iteration start ---");
            // --- Decode Next Frame (Owned) ---
            let decode_result = decoder.decode_next_frame_owned(&mut shutdown_rx).await;
            // Adjust trace log for new enum structure
            trace!(target: LOG_TARGET, "Decoder result: {:?}", decode_result.as_ref().map(|r| match r { DecodeRefResult::DecodedOwned(buf_ts) => format!("DecodedOwned(type={:?}, ts={})", buf_ts.type_id(), buf_ts.timestamp()), DecodeRefResult::EndOfStream => "EndOfStream".to_string(), DecodeRefResult::Skipped(s) => format!("Skipped({})", s), DecodeRefResult::Shutdown => "Shutdown".to_string() }).map_err(|e| format!("{:?}", e)));

            match decode_result {
                Ok(DecodeRefResult::DecodedOwned(decoded_buffer_ts)) => {
                    // Extract buffer and timestamp based on the enum variant
                    // Prefix unused `ts` with underscore in match arms
                    let (num_channels, _current_ts, s16_vec) = match decoded_buffer_ts {
                        DecodedBufferAndTimestamp::U8(audio_buffer, ts) => {
                            let nc = audio_buffer.spec().channels.count();
                            let vec = self._process_buffer(audio_buffer, ts, &pb, track_time_base, &mut last_progress_update_time).await?;
                            (nc, ts, vec)
                        }
                        DecodedBufferAndTimestamp::S16(audio_buffer, ts) => {
                            let nc = audio_buffer.spec().channels.count();
                            let vec = self._process_buffer(audio_buffer, ts, &pb, track_time_base, &mut last_progress_update_time).await?;
                            (nc, ts, vec)
                        }
                        DecodedBufferAndTimestamp::S24(audio_buffer, ts) => {
                            let nc = audio_buffer.spec().channels.count();
                            let vec = self._process_buffer(audio_buffer, ts, &pb, track_time_base, &mut last_progress_update_time).await?;
                            (nc, ts, vec)
                        }
                        DecodedBufferAndTimestamp::S32(audio_buffer, ts) => {
                            let nc = audio_buffer.spec().channels.count();
                            let vec = self._process_buffer(audio_buffer, ts, &pb, track_time_base, &mut last_progress_update_time).await?;
                            (nc, ts, vec)
                        }
                        DecodedBufferAndTimestamp::F32(audio_buffer, ts) => {
                            let nc = audio_buffer.spec().channels.count();
                            let vec = self._process_buffer(audio_buffer, ts, &pb, track_time_base, &mut last_progress_update_time).await?;
                            (nc, ts, vec)
                        }
                        DecodedBufferAndTimestamp::F64(audio_buffer, ts) => {
                            let nc = audio_buffer.spec().channels.count();
                            let vec = self._process_buffer(audio_buffer, ts, &pb, track_time_base, &mut last_progress_update_time).await?;
                            (nc, ts, vec)
                        }
                    };

                    // If processing returned None (e.g., conversion/resampling error), skip to next iteration
                    let s16_vec = match s16_vec {
                        Some(vec) => vec,
                        None => continue, // Skip this iteration if processing failed
                    };
                    // --- Check if buffer is empty after potential resampling/conversion ---
                    if s16_vec.is_empty() {
                        trace!(target: LOG_TARGET, "Skipping empty buffer after conversion/resampling.");
                        continue;
                    }

                    // --- ALSA Playback ---
                    // Use the original num_channels, as resampling preserves channel count
                    trace!(target: LOG_TARGET, "Calling _write_to_alsa with {} interleaved frames...", s16_vec.len() / num_channels);
                    if let Err(e) = self._write_to_alsa(&s16_vec, num_channels, &mut shutdown_rx).await {
                        pb.abandon_with_message(format!("ALSA Write Error: {}", e));
                        return Err(e);
                    }
                }
                Ok(DecodeRefResult::Skipped(reason)) => {
                     warn!(target: LOG_TARGET, "Decoder skipped packet: {}", reason);
                     continue;
                }
                Ok(DecodeRefResult::EndOfStream) => {
                    info!(target: LOG_TARGET, "Decoder reached end of stream.");
                    trace!(target: LOG_TARGET, "Breaking playback loop due to EndOfStream.");
                    pb.finish_with_message("Playback finished");
                    break 'decode_loop;
                }
                 Ok(DecodeRefResult::Shutdown) => {
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
        debug!(target: LOG_TARGET, "HTTP Response Headers: {:#?}", response.headers());
        let content_length = response.content_length();
        debug!(target: LOG_TARGET, "HTTP response received. Content-Length: {:?}", content_length);
        let stream = response.bytes_stream();
        let source = Box::new(ReqwestStreamWrapper::new_async(stream).await?);
        let mss = MediaSourceStream::new(source, MediaSourceStreamOptions { buffer_len: 64 * 1024 }); // Use default buffer size (64KB)
        // --- Symphonia Decoder Setup ---
        let decoder = SymphoniaDecoder::new(mss)?; // Removed mut
        let initial_spec = decoder.initial_spec().ok_or(AudioError::InitializationError("Decoder failed to provide initial spec".to_string()))?;
        let track_time_base = decoder.time_base();
// --- ALSA Initialization & Get Actual Rate ---
let actual_rate = {
    let mut handler_guard = self.alsa_handler.lock().map_err(|e| AudioError::InvalidState(format!("ALSA handler mutex poisoned on init: {}", e)))?;
    handler_guard.initialize(initial_spec)?;
    handler_guard.get_actual_rate().ok_or_else(|| AudioError::InitializationError("ALSA handler did not return actual rate after initialization".to_string()))?
};
debug!(target: LOG_TARGET, "Decoder rate: {}, ALSA actual rate: {}", initial_spec.rate, actual_rate);

// --- Resampler Setup (Conditional) ---
let decoder_rate = initial_spec.rate;
if decoder_rate != actual_rate {
    info!(target: LOG_TARGET, "Sample rate mismatch (Decoder: {}, ALSA: {}). Initializing resampler.", decoder_rate, actual_rate);
    let params = SincInterpolationParameters {
        sinc_len: 256, // Quality parameter, higher is better but slower
        f_cutoff: 0.95, // Cutoff frequency relative to Nyquist
        interpolation: SincInterpolationType::Linear, // Faster interpolation
        oversampling_factor: 256, // Higher means better quality
        window: WindowFunction::BlackmanHarris2, // Good quality window function
    };
    let chunk_size = 1024; // Process in chunks
    let resampler = SincFixedIn::<f32>::new(
        actual_rate as f64 / decoder_rate as f64, // ratio = target_rate / source_rate
        2.0, // max_resample_ratio_difference - allow some flexibility
        params,
        chunk_size,
        initial_spec.channels.count(), // Number of channels
    ).map_err(|e| AudioError::InitializationError(format!("Failed to create resampler: {}", e)))?;
    self.resampler = Some(Arc::new(TokioMutex::new(resampler)));
} else {
    info!(target: LOG_TARGET, "Sample rates match ({} Hz). Resampling disabled.", actual_rate);
    self.resampler = None;
}

        // --- Progress Bar & Info Setup ---
        let pb = self._setup_progress(content_length, track_time_base, Some(initial_spec), total_duration_ticks).await?;

        // --- Call Non-Generic Playback Loop ---
        // The loop now handles different buffer types internally.
        info!(target: LOG_TARGET, "Starting playback loop (handles format internally)...");
        let loop_result = self.playback_loop(decoder, pb, shutdown_rx).await;

        // --- End Playback Loop ---
        loop_result
    }


    /// Performs graceful asynchronous shutdown of the playback orchestrator.
    /// This should be called explicitly before dropping the orchestrator to ensure
    /// potentially blocking cleanup operations (like ALSA drain/close) complete.
    pub async fn shutdown(&mut self) -> Result<(), AudioError> {
        info!(target: LOG_TARGET, "Shutting down PlaybackOrchestrator asynchronously.");

        // 1. Abandon Progress Bar (Non-blocking)
        if let Some(pb) = self.progress_bar.take() {
            if !pb.is_finished() {
                pb.abandon_with_message("Playback stopped");
                debug!(target: LOG_TARGET, "Abandoned progress bar during shutdown.");
            }
        }

        // 2. Reset Progress Info (Async lock)
        if let Some(progress_mutex) = self.progress_info.take() { // Take ownership
            debug!(target: LOG_TARGET, "Resetting shared progress info...");
            let mut info = progress_mutex.lock().await; // Async lock
            *info = PlaybackProgressInfo::default();
            debug!(target: LOG_TARGET, "Reset shared progress info.");
            // Lock guard drops automatically here
        }

        // 3. Close ALSA Handler (Blocking operation in spawn_blocking)
        let alsa_handler_clone = Arc::clone(&self.alsa_handler);
        debug!(target: LOG_TARGET, "Spawning blocking task for ALSA close...");
        let close_result = task::spawn_blocking(move || {
            debug!(target: LOG_TARGET, "Executing blocking ALSA close operation...");
            match alsa_handler_clone.lock() { // std::sync::Mutex lock
                Ok(mut guard) => {
                    guard.close(); // This might block (e.g., drain)
                    debug!(target: LOG_TARGET, "ALSA handler closed in blocking task.");
                    Ok(()) // Indicate success
                }
                Err(poisoned) => {
                    error!(target: LOG_TARGET, "ALSA handler mutex poisoned during close: {}", poisoned);
                    Err(AudioError::InvalidState("ALSA handler mutex poisoned during close".to_string()))
                }
            }
        }).await; // Await the JoinHandle

        // Handle potential errors from spawn_blocking (task panic or returned error)
        match close_result {
            Ok(Ok(())) => {
                debug!(target: LOG_TARGET, "Blocking ALSA close task completed successfully.");
            }
            Ok(Err(e)) => {
                error!(target: LOG_TARGET, "Error returned from blocking ALSA close task: {}", e);
                // Propagate the error if needed, or just log it
                return Err(e);
            }
            Err(join_error) => {
                error!(target: LOG_TARGET, "Blocking ALSA close task panicked: {}", join_error);
                return Err(AudioError::TaskJoinError(join_error.to_string()));
            }
        }

        // 4. Clear Resampler (Non-blocking)
        self.resampler = None;
        debug!(target: LOG_TARGET, "Cleared resampler.");


        info!(target: LOG_TARGET, "PlaybackOrchestrator shutdown complete.");
        Ok(())
    }


    /// Original close method, now simplified for synchronous cleanup (called by Drop).
    /// This should perform minimal, non-blocking cleanup. The main cleanup
    /// is now handled by the async `shutdown` method.
    fn close(&mut self) {
        info!(target: LOG_TARGET, "Executing synchronous close (called from Drop). Minimal cleanup.");
        // Only perform actions here that are safe and necessary in a synchronous drop context.
        // For example, clearing references that don't involve blocking I/O.
        self.resampler = None;

        // We might choose to abandon the progress bar here too, as it's non-blocking.
        if let Some(pb) = self.progress_bar.take() {
             if !pb.is_finished() {
                 pb.abandon_with_message("Playback stopped (implicit drop)");
                 debug!(target: LOG_TARGET, "Abandoned progress bar during synchronous close/drop.");
             }
        }
        // DO NOT attempt to lock/close the ALSA handler here.
        // DO NOT attempt to lock/reset the async progress_info here.
    }
    // Helper function moved inside impl block
    async fn _process_buffer<S: Sample + std::fmt::Debug + Send + Sync + 'static>(
        &mut self,
        audio_buffer: symphonia::core::audio::AudioBuffer<S>,
        current_ts: u64,
        pb: &ProgressBar,
        track_time_base: Option<TimeBase>,
        last_progress_update_time: &mut Instant,
    ) -> Result<Option<Vec<i16>>, AudioError> { // Return Option<Vec<i16>>
        trace!(target: LOG_TARGET, "Processing buffer: {} frames, ts={}", audio_buffer.frames(), current_ts);

        // --- Progress Update ---
        self._update_progress(pb, current_ts, track_time_base, last_progress_update_time).await;

        let s16_vec: Vec<i16>;

        // --- Resampling Logic ---
        if let Some(resampler_arc) = self.resampler.as_ref() {
            let mut resampler = resampler_arc.lock().await;
            trace!(target: LOG_TARGET, "Resampling buffer...");
            let f32_input_vecs = match self._convert_buffer_to_f32_vecs(audio_buffer) {
                Ok(vecs) => vecs,
                Err(e) => {
                    warn!(target: LOG_TARGET, "Failed to convert buffer to F32 for resampling: {}. Skipping buffer.", e);
                    return Ok(None); // Indicate skip
                }
            };

            match resampler.process(&f32_input_vecs, None) {
                Ok(f32_output_vecs) => {
                    trace!(target: LOG_TARGET, "Resampling successful, got {} output frames.", f32_output_vecs.get(0).map_or(0, |v| v.len()));
                    s16_vec = match self._convert_f32_vecs_to_s16(f32_output_vecs) {
                        Ok(vec) => vec,
                        Err(e) => {
                            warn!(target: LOG_TARGET, "Failed to convert resampled F32 buffer to S16: {}. Skipping buffer.", e);
                            return Ok(None); // Indicate skip
                        }
                    };
                }
                Err(e) => {
                    error!(target: LOG_TARGET, "Resampling failed: {}", e);
                    return Ok(None); // Indicate skip
                }
            }
        } else { // No resampler needed
            trace!(target: LOG_TARGET, "No resampling needed, converting directly to S16...");
            s16_vec = match self._convert_buffer_to_s16(audio_buffer) {
                Ok(vec) => vec,
                Err(e) => {
                    warn!(target: LOG_TARGET, "Failed to convert buffer to S16: {}. Skipping buffer.", e);
                    return Ok(None); // Indicate skip
                }
            };
        }

        // --- Check if buffer is empty ---
        if s16_vec.is_empty() {
            trace!(target: LOG_TARGET, "Skipping empty buffer after conversion/resampling.");
            return Ok(None); // Indicate skip
        }

        Ok(Some(s16_vec)) // Return the processed S16 buffer
    }
} // End impl PlaybackOrchestrator


impl Drop for PlaybackOrchestrator {
    fn drop(&mut self) {
        // IMPORTANT: Avoid calling potentially blocking or async operations here.
        // Rely on the explicit `shutdown()` method for proper cleanup.
        // If `shutdown()` was not called, resources like the ALSA handler
        // might not be cleaned up gracefully, but calling blocking code here
        // can lead to panics or deadlocks.
        debug!(target: LOG_TARGET, "Dropping PlaybackOrchestrator. Explicit shutdown() is recommended for graceful cleanup.");
        // We can call the *simplified* synchronous self.close() if it only does non-blocking things.
        self.close();
    }
}

// Helper extension trait for DecodedBufferAndTimestamp
trait DecodedBufferTimestampExt {
    fn type_id(&self) -> TypeId;
    fn timestamp(&self) -> u64;
}

impl DecodedBufferTimestampExt for DecodedBufferAndTimestamp {
    fn type_id(&self) -> TypeId {
        match self {
            DecodedBufferAndTimestamp::U8(_, _) => TypeId::of::<u8>(),
            DecodedBufferAndTimestamp::S16(_, _) => TypeId::of::<i16>(),
            DecodedBufferAndTimestamp::S24(_, _) => TypeId::of::<i32>(), // S24 uses i32
            DecodedBufferAndTimestamp::S32(_, _) => TypeId::of::<i32>(),
            DecodedBufferAndTimestamp::F32(_, _) => TypeId::of::<f32>(),
            DecodedBufferAndTimestamp::F64(_, _) => TypeId::of::<f64>(),
        }
    }

    fn timestamp(&self) -> u64 {
        match self {
            DecodedBufferAndTimestamp::U8(_, ts) => *ts,
            DecodedBufferAndTimestamp::S16(_, ts) => *ts,
            DecodedBufferAndTimestamp::S24(_, ts) => *ts,
            DecodedBufferAndTimestamp::S32(_, ts) => *ts,
            DecodedBufferAndTimestamp::F32(_, ts) => *ts,
            DecodedBufferAndTimestamp::F64(_, ts) => *ts,
        }
    }
}
