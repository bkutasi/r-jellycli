use alsa::pcm::{Access, Format, HwParams, PCM};
use alsa::{Direction, ValueOr};
use alsa::nix::errno::Errno;
use bytes::Bytes;
use futures_util::StreamExt;
use indicatif::{ProgressBar, ProgressStyle};
use libc;
use log::{debug, error, info, warn};
use reqwest::Client;
use std::error::Error;
use std::ffi::CString;
use std::io;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration as StdDuration;
use symphonia::core::audio::{AudioBufferRef, SignalSpec};
// Remove SampleS24 import as it's causing issues. Will handle S24 explicitly.
use symphonia::core::codecs::{Decoder, DecoderOptions};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::{FormatOptions, FormatReader};
use symphonia::core::io::{MediaSource, MediaSourceStream};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use symphonia::core::units::TimeBase; // Correct path for TimeBase is likely units::TimeBase

/// Error types specific to audio playback
#[derive(Debug)]
pub enum AudioError {
    AlsaError(String),
    StreamError(String),
    DecodingError(String),
    SymphoniaError(SymphoniaError),
    IoError(io::Error),
    NetworkError(reqwest::Error),
    InvalidState(&'static str),
    UnsupportedFormat(&'static str),
    MissingCodecParams(&'static str),
    TaskJoinError(String),
}

impl std::fmt::Display for AudioError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AudioError::AlsaError(e) => write!(f, "ALSA error: {}", e),
            AudioError::StreamError(e) => write!(f, "Streaming error: {}", e),
            AudioError::DecodingError(e) => write!(f, "Decoding error: {}", e),
            AudioError::SymphoniaError(e) => write!(f, "Symphonia error: {}", e),
            AudioError::IoError(e) => write!(f, "I/O error: {}", e),
            AudioError::NetworkError(e) => write!(f, "Network error: {}", e),
            AudioError::InvalidState(s) => write!(f, "Invalid state: {}", s),
            AudioError::UnsupportedFormat(s) => write!(f, "Unsupported format: {}", s),
            AudioError::MissingCodecParams(s) => write!(f, "Missing codec parameters: {}", s),
            AudioError::TaskJoinError(e) => write!(f, "Async task join error: {}", e),
        }
    }
}

impl Error for AudioError {}

impl From<alsa::Error> for AudioError {
    fn from(e: alsa::Error) -> Self {
        AudioError::AlsaError(e.to_string())
    }
}

impl From<SymphoniaError> for AudioError {
    fn from(e: SymphoniaError) -> Self {
        AudioError::SymphoniaError(e)
    }
}

impl From<io::Error> for AudioError {
    fn from(e: io::Error) -> Self {
        AudioError::IoError(e)
    }
}

impl From<reqwest::Error> for AudioError {
    fn from(e: reqwest::Error) -> Self {
        AudioError::NetworkError(e)
    }
}

impl From<tokio::task::JoinError> for AudioError {
    fn from(e: tokio::task::JoinError) -> Self {
        AudioError::TaskJoinError(e.to_string())
    }
}

// --- Shared Progress State ---
#[derive(Debug, Default, Clone)]
pub struct PlaybackProgressInfo {
    pub current_seconds: f64,
    pub total_seconds: Option<f64>,
}

// --- Helper Function for Format Conversion ---
/// Converts various Symphonia audio buffer formats to interleaved S16 samples.
fn convert_to_s16(audio_buf_ref: &AudioBufferRef) -> Vec<i16> {
    let spec = audio_buf_ref.spec();
    let num_frames = audio_buf_ref.frames();
    let num_channels = spec.channels.count();
    let mut s16_vec = vec![0i16; num_frames * num_channels]; // Pre-allocate with zeros

    // Helper macro to handle interleaving for different sample types
    macro_rules! interleave {
        ($planes:expr, $frame:ident, $ch:ident, $sample_type:ty, $conversion_expr:expr) => {
            let channel_planes = $planes.planes();
            if num_channels == 1 {
                let channel_data = channel_planes[0];
                for $frame in 0..num_frames {
                    let sample: $sample_type = channel_data[$frame];
                    s16_vec[$frame] = $conversion_expr(sample);
                }
            } else {
                for $frame in 0..num_frames {
                    for $ch in 0..num_channels {
                        let sample: $sample_type = channel_planes[$ch as usize][$frame];
                        let idx = $frame * num_channels + $ch;
                        s16_vec[idx] = $conversion_expr(sample);
                    }
                }
            }
        };
    }

    match audio_buf_ref {
        AudioBufferRef::S16(buf) => {
            let planes = buf.planes(); // Bind to extend lifetime
            interleave!(planes, frame, ch, i16, |s: i16| s);
        }
        AudioBufferRef::F32(buf) => {
            let planes = buf.planes(); // Bind to extend lifetime
            interleave!(planes, frame, ch, f32, |s: f32| (s * 32767.0).clamp(-32768.0, 32767.0) as i16);
        }
        AudioBufferRef::U8(buf) => {
            let planes = buf.planes(); // Bind to extend lifetime
            interleave!(planes, frame, ch, u8, |s: u8| ((s as i16 - 128) * 256));
        }
        AudioBufferRef::S32(buf) => {
            let planes = buf.planes(); // Bind to extend lifetime
            interleave!(planes, frame, ch, i32, |s: i32| (s >> 16) as i16);
        }
        // Handle S24 explicitly, mirroring original logic without the macro
        AudioBufferRef::S24(buf) => {
            let planes = buf.planes();
            let channel_planes = planes.planes();
            if num_channels == 1 {
                let channel_data = channel_planes[0];
                for i in 0..num_frames {
                    // Access .0 field of the inferred sample type
                    s16_vec[i] = (channel_data[i].0 >> 8) as i16;
                }
            } else {
                for frame in 0..num_frames {
                    for ch in 0..num_channels {
                        // Access .0 field of the inferred sample type
                        let sample = channel_planes[ch as usize][frame];
                        let idx = frame * num_channels + ch;
                        s16_vec[idx] = (sample.0 >> 8) as i16;
                    }
                }
            }
        }
        _ => {
            warn!("[AUDIO] Unsupported audio format for conversion: {:?}. Playing silence.", spec);
            // s16_vec is already initialized to zeros
        }
    }
    s16_vec
}


/// Manages ALSA audio playback
pub struct AlsaPlayer {
    device_name: String,
    pcm: Option<PCM>,
    sample_rate: u32,
    channels: u32,
    progress_bar: Option<Arc<ProgressBar>>,
    progress_info: Option<Arc<StdMutex<PlaybackProgressInfo>>>,
}

impl AlsaPlayer {
    /// Create a new ALSA player
    pub fn new(device_name: &str) -> Self {
        info!("[AUDIO] Creating new AlsaPlayer for device: {}", device_name);
        AlsaPlayer {
            device_name: device_name.to_string(),
            pcm: None,
            sample_rate: 44100, // Default, will be updated
            channels: 2,      // Default, will be updated
            progress_bar: None,
            progress_info: None,
        }
    }

    /// Set the shared progress tracker.
    pub fn set_progress_tracker(&mut self, tracker: Arc<StdMutex<PlaybackProgressInfo>>) {
        info!("[AUDIO] Progress tracker configured for AlsaPlayer.");
        self.progress_info = Some(tracker);
    }

    /// Initialize ALSA playback device with specific parameters.
    fn initialize_with_params(&mut self, rate: u32, channels: u32) -> Result<(), AudioError> {
        info!(
            "[AUDIO] Initializing ALSA with params: rate={}, channels={}",
            rate, channels
        );
        self.sample_rate = rate;
        self.channels = channels;

        self.close_pcm(); // Close existing PCM if any

        let device = CString::new(self.device_name.clone())
            .map_err(|e| AudioError::AlsaError(format!("Invalid device name: {}", e)))?;

        let pcm = PCM::open(&device, Direction::Playback, false)?; // Use blocking mode for simplicity here

        { // Scope for HwParams
            let hwp = HwParams::any(&pcm)?;
            hwp.set_access(Access::RWInterleaved)?;
            hwp.set_format(Format::s16())?; // We convert everything to S16LE
            hwp.set_channels(self.channels)?;

            match hwp.set_rate_near(self.sample_rate, ValueOr::Nearest) {
                 Ok(_) => debug!("[AUDIO] Set ALSA rate near {}", self.sample_rate),
                 Err(e) => {
                     warn!("[AUDIO] Failed to set ALSA rate near {}: {}", self.sample_rate, e);
                     return Err(AudioError::AlsaError(format!("Failed to set sample rate {}: {}", self.sample_rate, e)));
                 }
            }

            pcm.hw_params(&hwp)?; // Apply hardware parameters

            // Check actual rate
            let actual_rate = hwp.get_rate()?;
            if actual_rate != self.sample_rate {
                warn!("[AUDIO] Actual ALSA rate {} differs from requested {}", actual_rate, self.sample_rate);
                // Consider updating self.sample_rate = actual_rate; if needed
            } else {
                 debug!("[AUDIO] ALSA rate set successfully to {}", actual_rate);
            }

            // Software parameters (optional but good practice)
            let swp = pcm.sw_params_current()?;
            let buffer_size = hwp.get_buffer_size()?;
            let period_size = hwp.get_period_size()?;
            swp.set_start_threshold(buffer_size - period_size)?;
            // Consider setting avail_min if needed for low latency
            pcm.sw_params(&swp)?;
        } // HwParams dropped here

        self.pcm = Some(pcm);
        info!("[AUDIO] ALSA Initialized successfully.");
        Ok(())
    }

    /// Play audio buffer (S16LE interleaved), attempting recovery on underrun.
    /// Returns Ok(frames_written) on success, or Err(AudioError::AlsaError("EPIPE".to_string()))
    /// if an underrun occurred and was recovered from. Other errors are returned directly.
    fn play_s16_buffer(&self, buffer: &[i16]) -> Result<usize, AudioError> {
        match &self.pcm {
            Some(pcm) => {
                let io = pcm.io_i16()?; // Get IO interface
                match io.writei(buffer) {
                    Ok(frames_written) => Ok(frames_written),
                    Err(e) => {
                        if e.errno() == Errno::EPIPE { // Check for underrun
                            warn!("[AUDIO] ALSA buffer underrun (EPIPE), attempting recovery...");
                            match pcm.recover(libc::EPIPE, true) { // Attempt recovery (blocking)
                                Ok(()) => { // pcm.recover returns Ok(()) on success
                                    debug!("[AUDIO] ALSA recovery successful.");
                                    // Indicate underrun occurred and was recovered
                                    Err(AudioError::AlsaError("EPIPE".to_string()))
                                }
                                // The Ok(n) case is unreachable as recover returns Ok(()) or Err
                                // Err(recover_err) is handled below
                                // Let's remove the redundant Ok(n) arm for clarity if needed,
                                // but the primary fix is changing Ok(0) to Ok(())
                                // For now, just fixing the type and log message:
                                // Ok(n) => { // This arm might be technically unreachable now
                                //      warn!("[AUDIO] ALSA recovery returned unexpected value: {:?}", n); // Use debug format for ()
                                //      Err(AudioError::AlsaError(format!("ALSA recovery failed (ret={:?})", n))) // Use debug format for ()
                                // }
                                Err(recover_err) => {
                                    error!("[AUDIO] ALSA recovery failed: {}", recover_err);
                                    Err(AudioError::AlsaError(format!("ALSA recovery failed: {}", recover_err)))
                                }
                            }
                        } else {
                            error!("[AUDIO] ALSA write error: {}", e);
                            Err(AudioError::AlsaError(e.to_string())) // Other ALSA errors
                        }
                    }
                }
            }
            None => Err(AudioError::InvalidState("PCM not initialized")),
        }
    }

    /// The main loop for decoding packets and sending them to ALSA.
    async fn playback_loop(
        &mut self,
        mut format_reader: Box<dyn FormatReader>,
        mut decoder: Box<dyn Decoder>,
        track_id: u32,
        track_time_base: Option<TimeBase>,
        initial_spec: SignalSpec,
        pb: Arc<ProgressBar>,
        mut shutdown_rx: tokio::sync::broadcast::Receiver<()>,
    ) -> Result<(), AudioError> {

        let mut last_progress_update_time = std::time::Instant::now();
        const PROGRESS_UPDATE_INTERVAL: StdDuration = StdDuration::from_millis(500);
        let total_seconds = self.progress_info.as_ref()
            .and_then(|pi| pi.lock().ok())
            .and_then(|info| info.total_seconds);

        'decode_loop: loop {
            // --- Read Next Packet ---
            let next_packet_result = tokio::select! {
                biased; // Prioritize shutdown check

                _ = shutdown_rx.recv() => {
                    info!("[AUDIO] Shutdown signal received during packet reading. Stopping playback.");
                    return Ok(()); // Graceful exit on shutdown
                }

                // Use spawn_blocking for the synchronous next_packet call
                maybe_packet_res = tokio::task::spawn_blocking(move || {
                    // Move reader and decoder into the blocking task
                    let packet_res = format_reader.next_packet();
                    // Return ownership along with the result
                    (format_reader, decoder, packet_res)
                }) => {
                    match maybe_packet_res {
                        Ok((ret_reader, ret_decoder, Ok(packet))) => {
                            // Restore ownership
                            format_reader = ret_reader;
                            decoder = ret_decoder;
                            Ok(packet) // Packet successfully read
                        },
                        Ok((ret_reader, ret_decoder, Err(e))) => {
                            // Restore ownership even on error
                            format_reader = ret_reader;
                            decoder = ret_decoder;
                            Err(e) // Error reading packet
                        },
                        Err(join_error) => {
                             // Task failed, reader/decoder ownership is lost
                             error!("[AUDIO] Spawn blocking task for next_packet failed: {}", join_error);
                             return Err(AudioError::TaskJoinError(join_error.to_string()));
                        }
                    }
                }
            };
            // --- End Read Next Packet ---


            match next_packet_result {
                Ok(packet) => {
                    if packet.track_id() != track_id { continue; } // Skip packets for other tracks

                    // --- Decode Packet ---
                    match decoder.decode(&packet) {
                        Ok(audio_buf_ref) => {
                            let current_packet_ts = packet.ts();

                            // --- Progress Update ---
                            if let Some(tb) = track_time_base {
                                let current_time = tb.calc_time(current_packet_ts);
                                let current_time_seconds = current_time.seconds as f64 + current_time.frac;

                                if total_seconds.is_some() {
                                    pb.set_position(current_time_seconds.floor() as u64);
                                }
                                // TODO: Update progress based on bytes if total_seconds is None?

                                if last_progress_update_time.elapsed() >= PROGRESS_UPDATE_INTERVAL {
                                    if let Some(progress_mutex) = &self.progress_info {
                                        if let Ok(mut info) = progress_mutex.lock() {
                                            info.current_seconds = current_time_seconds;
                                        } else {
                                            error!("[AUDIO] Failed to lock progress info mutex for update.");
                                        }
                                    }
                                    last_progress_update_time = std::time::Instant::now();
                                }
                            }
                            // --- End Progress Update ---

                            if audio_buf_ref.spec() != &initial_spec {
                                warn!("[AUDIO] Specification changed mid-stream (rate: {}, channels: {:?})",
                                      audio_buf_ref.spec().rate, audio_buf_ref.spec().channels);
                                // Consider re-initializing ALSA or handling differently
                            }

                            // --- Format Conversion ---
                            let s16_vec = convert_to_s16(&audio_buf_ref);
                            // --- End Format Conversion ---

                            // --- ALSA Playback ---
                            let num_channels = audio_buf_ref.spec().channels.count();
                            if num_channels == 0 { continue; } // Skip if no channels
                            let frames_to_play = s16_vec.len() / num_channels;
                            let mut offset = 0;

                            while offset < frames_to_play {
                                // Check for shutdown signal *before* potentially blocking write
                                if shutdown_rx.try_recv().is_ok() {
                                     info!("[AUDIO] Shutdown signal received during ALSA write loop.");
                                     return Ok(());
                                }

                                let buffer_slice = &s16_vec[offset * num_channels..];
                                match self.play_s16_buffer(buffer_slice) {
                                    Ok(frames_written) => {
                                        if frames_written > 0 {
                                            offset += frames_written;
                                        } else {
                                            // ALSA might return 0 if buffer is full but not an error
                                            // Yield to allow other tasks to run, then retry writing
                                            tokio::task::yield_now().await;
                                        }
                                    }
                                    Err(AudioError::AlsaError(ref e_str)) if e_str == "EPIPE" => {
                                        // Underrun occurred and was recovered from in play_s16_buffer
                                        warn!("[AUDIO] Resuming playback after recovered underrun.");
                                        // Skip the rest of this decoded buffer to avoid potential glitches
                                        offset = frames_to_play;
                                    }
                                    Err(e) => { // Handle other ALSA errors
                                        error!("[AUDIO] ALSA write error: {}", e);
                                        pb.abandon_with_message(format!("ALSA Write Error: {}", e));
                                        return Err(e);
                                    }
                                }
                            }
                            // --- End ALSA Playback ---
                        }
                        Err(SymphoniaError::DecodeError(err)) => {
                            warn!("[AUDIO] Decode error: {}", err); // Log decode errors but continue
                        }
                        Err(e) => { // Other decoder errors
                            error!("[AUDIO] Unexpected decoder error: {}", e);
                            pb.abandon_with_message(format!("Decoder Error: {}", e));
                            return Err(e.into());
                        }
                    }
                    // --- End Decode Packet ---
                }
                Err(SymphoniaError::IoError(err)) if err.kind() == io::ErrorKind::UnexpectedEof => {
                    info!("[AUDIO] End of stream reached.");
                    pb.finish_with_message("Playback finished");
                    break 'decode_loop; // Normal end of stream
                }
                Err(SymphoniaError::ResetRequired) => {
                    warn!("[AUDIO] Symphonia decoder reset required (unhandled).");
                    pb.abandon_with_message("Stream discontinuity (ResetRequired)");
                    break 'decode_loop; // Stop playback on discontinuity
                }
                 Err(SymphoniaError::IoError(ref io_err)) if io_err.kind() == std::io::ErrorKind::Interrupted => {
                     // This might catch cancellations if the select! branch didn't
                     info!("[AUDIO] Playback interrupted (likely cancelled).");
                     pb.abandon_with_message("Playback cancelled");
                     return Ok(()); // Treat cancellation as success
                 }
                Err(e) => { // Handle other packet reading errors
                    error!("[AUDIO] Error reading packet: {}", e);
                    pb.abandon_with_message(format!("Stream Read Error: {}", e));
                    return Err(e.into());
                }
            }
        } // end 'decode_loop

        // Drain ALSA buffer after loop finishes successfully (EOF)
        if let Some(pcm) = &self.pcm {
            debug!("[AUDIO] Draining ALSA buffer after EOF.");
            match pcm.drain() {
                Ok(_) => debug!("[AUDIO] ALSA drain successful."),
                Err(e) => warn!("[AUDIO] Error draining ALSA buffer: {}", e),
            }
        }

        Ok(()) // Playback completed successfully
    }


    /// Stream audio from a URL, decode, play, and show progress.
    pub async fn stream_decode_and_play(
        &mut self,
        url: &str,
        total_duration_ticks: Option<i64>,
        shutdown_rx: tokio::sync::broadcast::Receiver<()>, // Takes ownership
    ) -> Result<(), AudioError> {
        info!("[AUDIO] Starting stream/decode/play for URL: {}", url);

        // --- HTTP Streaming Setup ---
        let client = Client::new();
        let response = client.get(url).send().await?.error_for_status()?;
        let content_length = response.content_length();
        let stream = response.bytes_stream();
        // Wrap the stream; pre-downloads all data currently
        let source = Box::new(ReqwestStreamWrapper::new_async(stream).await?);
        let mss = MediaSourceStream::new(source, Default::default());
        // --- End HTTP Streaming Setup ---

        // --- Symphonia Setup ---
        let hint = Hint::new();
        let meta_opts: MetadataOptions = Default::default();
        let fmt_opts: FormatOptions = Default::default();
        let probed = symphonia::default::get_probe()
            .format(&hint, mss, &fmt_opts, &meta_opts)?;
        let format_reader = probed.format; // Ownership taken

        let track = format_reader
            .tracks()
            .iter()
            .find(|t| t.codec_params.codec != symphonia::core::codecs::CODEC_TYPE_NULL)
            .ok_or(AudioError::UnsupportedFormat("No suitable audio track found"))?
            .clone(); // Clone track info before format_reader is moved

        let track_id = track.id;
        let track_params = track.codec_params; // Ownership taken
        let track_time_base = track_params.time_base;

        let initial_rate = track_params.sample_rate.ok_or(AudioError::MissingCodecParams("sample rate"))?;
        let initial_channels_map = track_params.channels.ok_or(AudioError::MissingCodecParams("channels map"))?;
        let initial_channels_count = initial_channels_map.count() as u32;
        let initial_spec = SignalSpec::new(initial_rate, initial_channels_map);

        let decoder = symphonia::default::get_codecs()
            .make(&track_params, &DecoderOptions::default())?; // Ownership taken
        // --- End Symphonia Setup ---

        // --- ALSA Initialization ---
        // This must happen *before* moving format_reader/decoder into playback_loop
        self.initialize_with_params(initial_rate, initial_channels_count)?;
        // --- End ALSA Initialization ---

        // --- Progress Bar & Info Setup ---
        let total_seconds = total_duration_ticks
            .and_then(|ticks| track_time_base.map(|tb| {
                let time = tb.calc_time(ticks as u64);
                time.seconds as f64 + time.frac
            }))
            .or_else(|| { // Estimate from content length if duration unknown
                content_length.and_then(|len| {
                    track_params.bits_per_sample.zip(track_params.sample_rate).map(|(bps, rate)| {
                        let bytes_per_sec = (rate * bps as u32 * initial_channels_count) / 8;
                        if bytes_per_sec > 0 { len as f64 / bytes_per_sec as f64 } else { 0.0 } // Removed parentheses
                    })
                })
            });

        // Update shared progress info with total duration if known
        if let Some(progress_mutex) = &self.progress_info {
             if let Ok(mut info) = progress_mutex.lock() {
                 info.total_seconds = total_seconds;
                 info.current_seconds = 0.0; // Reset current time
             } else {
                 error!("[AUDIO] Failed to lock progress info mutex for initialization.");
             }
        }

        let pb = Arc::new(ProgressBar::new(total_seconds.map(|s| s.ceil() as u64).unwrap_or(0)));
        if total_seconds.is_some() {
            pb.set_style(ProgressStyle::default_bar()
                .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] ({eta})")
                .expect("Invalid template string")
                .progress_chars("#>-"));
        } else {
             pb.set_style(ProgressStyle::default_spinner()
                .template("{spinner:.green} [{elapsed_precise}] {bytes}/{total_bytes} ({bytes_per_sec})") // Requires byte tracking
                .expect("Invalid template string"));
             if let Some(len) = content_length {
                 pb.set_length(len); // Set length for byte-based progress
             }
        }
        self.progress_bar = Some(pb.clone()); // Store clone for potential external access/closing
        // --- End Progress Bar & Info Setup ---


        // --- Call Playback Loop ---
        let loop_result = self.playback_loop(
            format_reader, // Move ownership
            decoder,       // Move ownership
            track_id,
            track_time_base,
            initial_spec,
            pb, // Pass the Arc<ProgressBar>
            shutdown_rx, // Move ownership
        ).await;
        // --- End Playback Loop ---

        // Cleanup (like closing progress bar) happens in Drop or explicit close()

        loop_result // Return the result from the loop
    }

    /// Close the PCM device if it's open, ensuring drain is attempted.
    fn close_pcm(&mut self) {
        if let Some(pcm) = self.pcm.take() { // Take ownership to drop
            debug!("[AUDIO] Closing ALSA PCM device...");
            match pcm.drain() {
                Ok(_) => debug!("[AUDIO] ALSA drain successful before closing."),
                Err(e) => warn!("[AUDIO] Error draining ALSA buffer during close: {}", e),
            }
            // PCM is dropped here
            debug!("[AUDIO] ALSA PCM closed.");
        }
    }

    /// Close the audio device and clean up resources like the progress bar.
    pub fn close(&mut self) {
        info!("[AUDIO] Closing AlsaPlayer resources.");
        self.close_pcm();
        if let Some(pb) = self.progress_bar.take() {
            if !pb.is_finished() {
                pb.abandon_with_message("Playback stopped"); // Or finish_and_clear()?
            }
        }
    }
}

impl Drop for AlsaPlayer {
    fn drop(&mut self) {
        // Ensure resources are released when AlsaPlayer goes out of scope
        debug!("[AUDIO] Dropping AlsaPlayer.");
        self.close();
    }
}

// --- Helper for Symphonia MediaSourceStream using Reqwest ---

/// Wraps a Reqwest byte stream, pre-downloading all content to satisfy `Read`.
/// Note: This defeats true streaming but simplifies `MediaSource` implementation.
struct ReqwestStreamWrapper {
    buffer: Vec<u8>,
    position: usize,
}

impl ReqwestStreamWrapper {
    /// Creates a new wrapper by asynchronously downloading the entire stream content.
    async fn new_async(stream: impl futures_util::Stream<Item = Result<Bytes, reqwest::Error>> + Send + Unpin + 'static) -> Result<Self, reqwest::Error> {
        let mut buffer = Vec::new();
        let mut stream = stream; // No need to Box::pin if already Unpin

        debug!("[AUDIO] Pre-downloading audio stream...");
        while let Some(chunk_result) = stream.next().await {
            let chunk = chunk_result?;
            buffer.extend_from_slice(&chunk);
        }
        debug!("[AUDIO] Pre-download complete ({} bytes).", buffer.len());

        Ok(Self {
            buffer,
            position: 0,
        })
    }
}

impl io::Read for ReqwestStreamWrapper {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let available = self.buffer.len().saturating_sub(self.position);
        if available == 0 {
            return Ok(0); // End of buffer
        }

        let to_read = std::cmp::min(buf.len(), available);
        buf[..to_read].copy_from_slice(&self.buffer[self.position..self.position + to_read]);
        self.position += to_read;

        Ok(to_read)
    }
}

impl io::Seek for ReqwestStreamWrapper {
    fn seek(&mut self, _pos: io::SeekFrom) -> io::Result<u64> {
        // Seeking is not supported as we buffer the entire stream.
        // Returning an error aligns with the non-seekable nature of HTTP streams.
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Seeking not supported on buffered HTTP stream",
        ))
    }
}

impl MediaSource for ReqwestStreamWrapper {
     fn is_seekable(&self) -> bool {
         false // HTTP streams are generally not seekable without Range requests
     }
     fn byte_len(&self) -> Option<u64> {
         // We have the full buffer, so we know the length.
         Some(self.buffer.len() as u64)
     }
 }

// These are necessary because ReqwestStreamWrapper contains Vec<u8> and usize,
// which are Send + Sync, but the compiler might not infer it across async boundaries
// without these explicit (but safe) declarations when used with MediaSourceStream.
unsafe impl Send for ReqwestStreamWrapper {}
unsafe impl Sync for ReqwestStreamWrapper {}
