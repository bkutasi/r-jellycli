    //! Audio playback implementation using ALSA with streaming and progress

use alsa::pcm::{Access, Format, HwParams, PCM};
use alsa::Direction;
use alsa::nix::errno::Errno;
use alsa::ValueOr;
use bytes::Bytes;
use indicatif::{ProgressBar, ProgressStyle};
use libc;
use reqwest::Client;
use std::error::Error;
use std::ffi::CString;
use std::io;
use std::sync::Arc;
use std::sync::Mutex;
use symphonia::core::audio::SignalSpec;
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::{MediaSource, MediaSourceStream};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use symphonia::core::codecs::DecoderOptions;

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

impl From<&str> for AudioError {
    fn from(s: &str) -> Self {
        AudioError::StreamError(s.to_string())
    }
}

/// Manages ALSA audio playback
pub struct AlsaPlayer {
    device_name: String,
    pcm: Option<PCM>,
    sample_rate: u32,
    channels: u32,
    progress_bar: Option<Arc<ProgressBar>>,
}

impl AlsaPlayer {
    /// Create a new ALSA player
    pub fn new(device_name: &str) -> Self {
        AlsaPlayer {
            device_name: device_name.to_string(),
            pcm: None,
            sample_rate: 44100,
            channels: 2,
            progress_bar: None,
        }
    }

    /// Initialize ALSA playback device with specific parameters
    fn initialize_with_params(&mut self, rate: u32, channels: u32) -> Result<(), AudioError> {
        // Force 48000Hz sample rate to match CamillaDSP configuration
        let forced_rate = 48000;
        println!(
            "[AUDIO-DEBUG] Initializing ALSA with params: rate={} (forced from {}), channels={}",
            forced_rate, rate, channels
        );
        self.sample_rate = forced_rate; // Use the forced rate
        self.channels = channels;

        self.close_pcm();

        let device = CString::new(self.device_name.clone())
            .map_err(|e| AudioError::AlsaError(format!("Invalid device name: {}", e)))?;
        
        // Open PCM device with non-blocking mode
        let pcm = PCM::open(&device, Direction::Playback, false)?;

        {
            let hwp = HwParams::any(&pcm)?;
            
            // Set hardware parameters - in this exact order for better compatibility
            hwp.set_access(Access::RWInterleaved)?;
            hwp.set_format(Format::s16())?;
            hwp.set_channels(self.channels)?;
            
            // Force rate to exactly 48000Hz as required by CamillaDSP
            match hwp.set_rate(self.sample_rate, ValueOr::Nearest) {
                Ok(_) => {},
                Err(e) => {
                    println!("[AUDIO-DEBUG] Failed to set exact rate: {}, trying nearest", e);
                    hwp.set_rate(self.sample_rate, ValueOr::Nearest)?
                }
            };
            
            // Apply hardware parameters
            match pcm.hw_params(&hwp) {
                Ok(_) => {},
                Err(e) => {
                    return Err(AudioError::AlsaError(format!(
                        "Failed to set hw params: {}. Try a different ALSA device.", e
                    )));
                }
            }

            // Get actual rate that was set
            let actual_rate = hwp.get_rate()?;
            if actual_rate != self.sample_rate {
                println!("[AUDIO-WARN] Actual rate {} differs from requested {}", 
                          actual_rate, self.sample_rate);
            }
            
            // Software parameters
            let swp = pcm.sw_params_current()?;
            swp.set_start_threshold(hwp.get_buffer_size()? - hwp.get_period_size()?)?;
            pcm.sw_params(&swp)?;
        }

        self.pcm = Some(pcm);
        println!("[AUDIO-DEBUG] ALSA Initialized successfully.");
        Ok(())
    }

    /// Play audio buffer (S16LE interleaved)
    fn play_s16_buffer(&self, buffer: &[i16]) -> Result<usize, AudioError> {
        match &self.pcm {
            Some(pcm) => {
                let io = pcm.io_i16()?;
                match io.writei(buffer) {
                    Ok(frames_written) => Ok(frames_written),
                    Err(e) => {
                        // Check if the error is EPIPE (Broken pipe), indicating buffer underrun
                        if e.errno() == Errno::EPIPE {
                            println!("[AUDIO-WARN] ALSA buffer underrun (EPIPE), attempting recovery.");
                            // Attempt to recover the PCM stream
                            // The second argument `true` means wait for the stream to be ready
                            pcm.recover(libc::EPIPE, true)?;
                            Ok(0) // Indicate 0 frames written after recovery attempt
                        } else {
                            // Handle other ALSA errors
                            println!("[AUDIO-ERROR] ALSA write error: {}", e);
                            Err(AudioError::AlsaError(e.to_string()))
                        }
                    }
                }
            }
            None => Err(AudioError::InvalidState("PCM not initialized")),
        }
    }

    /// Stream audio from a URL, decode, play, and show progress, allowing for cancellation.
    pub async fn stream_decode_and_play(
        &mut self,
        url: &str,
        total_duration_ticks: Option<i64>,
        mut shutdown_rx: tokio::sync::broadcast::Receiver<()>, // Add shutdown receiver
    ) -> Result<(), AudioError> {
        println!("[AUDIO-DEBUG] Starting stream_decode_and_play for URL: {}", url);

        let client = Client::new();
        let response = client.get(url).send().await?.error_for_status()?;
        let content_length = response.content_length();
        let stream = response.bytes_stream();

        // Pre-download the entire stream before starting playback to avoid async/sync issues
        let source = Box::new(ReqwestStreamWrapper::new_async(stream).await?);
        let mss = MediaSourceStream::new(source, Default::default());

        let hint = Hint::new();
        let meta_opts: MetadataOptions = Default::default();
        let fmt_opts: FormatOptions = Default::default();

        let probed = symphonia::default::get_probe()
            .format(&hint, mss, &fmt_opts, &meta_opts)?;

        let format_reader = Arc::new(Mutex::new(probed.format)); // Wrap in Arc<Mutex<>>

        // Lock the reader to find the track
        let track = format_reader
            .lock()
            .expect("Audio reader mutex poisoned during track finding")
            .tracks()
            .iter()
            .find(|t| t.codec_params.codec != symphonia::core::codecs::CODEC_TYPE_NULL)
            .cloned() // Clone the track data as the lock guard is temporary
            .ok_or(AudioError::UnsupportedFormat("No suitable audio track found"))?;

        let track_id = track.id;
        let track_params = track.codec_params.clone();
        let track_time_base = track_params.time_base;

        let initial_rate = track_params.sample_rate.ok_or(AudioError::MissingCodecParams("sample rate"))?;
        let initial_channels_map = track_params.channels.ok_or(AudioError::MissingCodecParams("channels map"))?;
        let initial_channels_count = initial_channels_map.count() as u32;
        self.initialize_with_params(initial_rate, initial_channels_count)?;
        let initial_spec = SignalSpec::new(initial_rate, initial_channels_map);

        let mut decoder = symphonia::default::get_codecs()
            .make(&track_params, &DecoderOptions::default())?;

        let total_seconds = total_duration_ticks
            .and_then(|ticks| track_time_base.map(|tb| tb.calc_time(ticks as u64).seconds))
            .or_else(|| {
                content_length.and_then(|len| {
                    track_params.bits_per_sample.zip(track_params.sample_rate).map(|(bps, rate)| {
                        let bytes_per_sec = (rate * bps as u32 * initial_channels_count) / 8;
                        if bytes_per_sec > 0 { len / bytes_per_sec as u64 } else { 0 }
                    })
                })
            });

        let pb = Arc::new(ProgressBar::new(total_seconds.unwrap_or(0)));
        if total_seconds.is_some() {
            pb.set_style(ProgressStyle::default_bar()
                .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] ({eta})")
                .expect("Invalid template string")
                .progress_chars("#>-"));
        } else {
             pb.set_style(ProgressStyle::default_spinner()
                .template("{spinner:.green} [{elapsed_precise}] {bytes}/{total_bytes} ({bytes_per_sec})")
                .expect("Invalid template string"));
             if let Some(len) = content_length {
                 pb.set_length(len);
             }
        }
        self.progress_bar = Some(pb.clone());

        let mut s16_vec: Vec<i16> = Vec::new();
        #[allow(unused_assignments)]
        let mut current_time_seconds = 0.0;

        'decode_loop: loop {
            // Select between getting the next packet and receiving a shutdown signal
            let next_packet_result = tokio::select! {
                biased; // Prioritize shutdown check slightly

                // Branch 1: Shutdown signal received
                _ = shutdown_rx.recv() => {
                    println!("[AUDIO-DEBUG] Shutdown signal received during packet reading.");
                    Err(SymphoniaError::IoError(std::io::Error::new(
                        std::io::ErrorKind::Interrupted,
                        "Playback cancelled by shutdown signal",
                    )))
                }

                // Branch 2: Get the next packet using spawn_blocking
                // Assuming format_reader is Arc<Mutex<Box<dyn FormatReader>>> or similar Send + Sync type
                maybe_packet_res = {
                    // Clone the Arc to move into the blocking task
                    let reader_clone = Arc::clone(&format_reader);
                    tokio::task::spawn_blocking(move || {
                        // Lock the mutex inside the blocking task
                        let mut reader_guard = reader_clone.lock().expect("Audio reader mutex poisoned");
                        reader_guard.next_packet() // Call next_packet on the guard
                    })
                } => { // Await the JoinHandle from spawn_blocking here
                    match maybe_packet_res {
                        // spawn_blocking finished successfully, returning the Result from next_packet()
                        Ok(Ok(packet)) => Ok(packet), // Inner Ok: next_packet succeeded
                        Ok(Err(symphonia_err)) => Err(symphonia_err), // Inner Err: next_packet failed
                        // spawn_blocking task failed (e.g., panicked or cancelled)
                        Err(join_error) => {
                            eprintln!("[AUDIO-ERROR] Spawn blocking task for next_packet failed: {}", join_error);
                            Err(SymphoniaError::IoError(std::io::Error::new(
                                std::io::ErrorKind::Other,
                                format!("Spawn blocking task failed: {}", join_error),
                            )))
                        }
                    }
                }
            };

            match next_packet_result {
                Ok(packet) => {
                    if packet.track_id() != track_id { continue; }

                    match decoder.decode(&packet) {
                        Ok(audio_buf_ref) => {
                            if let Some(tb) = track_time_base {
                                current_time_seconds = tb.calc_time(packet.ts()).seconds as f64 + tb.calc_time(packet.ts()).frac;
                                if total_seconds.is_some() {
                                    pb.set_position(current_time_seconds as u64);
                                }
                            }

                            if audio_buf_ref.spec() != &initial_spec {
                                println!("[AUDIO-WARN] Specification changed mid-stream (unhandled)");
                            }

                            let num_frames = audio_buf_ref.frames();
                            let num_channels = audio_buf_ref.spec().channels.count();
                            s16_vec.resize(num_frames * num_channels, 0);

                            // Convert any audio format to S16 for playback
                            match audio_buf_ref {
                                // Directly use S16 samples
                                symphonia::core::audio::AudioBufferRef::S16(buf) => {
                                    // Get interleaved samples directly
                                    if num_channels == 1 {
                                        // Fast path for mono audio
                                        // Get the audio planes to access the channel data
                                        let planes = buf.planes();
                                        let channel_data = planes.planes()[0];
                                        for i in 0..num_frames {
                                            s16_vec[i] = channel_data[i];
                                        }
                                    } else {
                                        // Multi-channel audio: interleave samples manually
                                        let planes = buf.planes();
                                        let channel_planes = planes.planes();
                                        for frame in 0..num_frames {
                                            for ch in 0..num_channels {
                                                let idx = frame * num_channels + ch;
                                                s16_vec[idx] = channel_planes[ch as usize][frame];
                                            }
                                        }
                                    }
                                },
                                // Convert F32 to S16
                                symphonia::core::audio::AudioBufferRef::F32(buf) => {
                                    // F32 has range [-1.0, 1.0], S16 has range [-32768, 32767]
                                    let planes = buf.planes();
                                    let channel_planes = planes.planes();
                                    
                                    if num_channels == 1 {
                                        // Fast path for mono
                                        let channel_data = channel_planes[0];
                                        for i in 0..num_frames {
                                            // Convert and clamp float to S16 range
                                            s16_vec[i] = (channel_data[i] * 32767.0) as i16;
                                        }
                                    } else {
                                        // Multi-channel
                                        for frame in 0..num_frames {
                                            for ch in 0..num_channels {
                                                let sample = channel_planes[ch as usize][frame];
                                                let idx = frame * num_channels + ch;
                                                s16_vec[idx] = (sample * 32767.0) as i16;
                                            }
                                        }
                                    }
                                },
                                // Convert U8 to S16
                                symphonia::core::audio::AudioBufferRef::U8(buf) => {
                                    // U8 has range [0, 255], S16 has range [-32768, 32767]
                                    let planes = buf.planes();
                                    let channel_planes = planes.planes();
                                    
                                    if num_channels == 1 {
                                        let channel_data = channel_planes[0];
                                        for i in 0..num_frames {
                                            // Convert U8 to S16 (center at 0)
                                            s16_vec[i] = ((channel_data[i] as i16 - 128) * 256) as i16;
                                        }
                                    } else {
                                        for frame in 0..num_frames {
                                            for ch in 0..num_channels {
                                                let sample = channel_planes[ch as usize][frame];
                                                let idx = frame * num_channels + ch;
                                                s16_vec[idx] = ((sample as i16 - 128) * 256) as i16;
                                            }
                                        }
                                    }
                                },
                                // Convert S24 to S16
                                symphonia::core::audio::AudioBufferRef::S24(buf) => {
                                    // S24 has range [-8388608, 8388607], S16 has range [-32768, 32767]
                                    let planes = buf.planes();
                                    let channel_planes = planes.planes();
                                    
                                    if num_channels == 1 {
                                        let channel_data = channel_planes[0];
                                        for i in 0..num_frames {
                                            // Scale down by 256 (2^8)
                                            s16_vec[i] = (channel_data[i].0 >> 8) as i16;
                                        }
                                    } else {
                                        for frame in 0..num_frames {
                                            for ch in 0..num_channels {
                                                let sample = channel_planes[ch as usize][frame];
                                                let idx = frame * num_channels + ch;
                                                s16_vec[idx] = (sample.0 >> 8) as i16;
                                            }
                                        }
                                    }
                                },
                                // Convert S32 to S16
                                symphonia::core::audio::AudioBufferRef::S32(buf) => {
                                    // S32 has range [-2147483648, 2147483647], S16 has range [-32768, 32767]
                                    let planes = buf.planes();
                                    let channel_planes = planes.planes();
                                    
                                    if num_channels == 1 {
                                        let channel_data = channel_planes[0];
                                        for i in 0..num_frames {
                                            // Scale down by 65536 (2^16)
                                            s16_vec[i] = (channel_data[i] >> 16) as i16;
                                        }
                                    } else {
                                        for frame in 0..num_frames {
                                            for ch in 0..num_channels {
                                                let sample = channel_planes[ch as usize][frame];
                                                let idx = frame * num_channels + ch;
                                                s16_vec[idx] = (sample >> 16) as i16;
                                            }
                                        }
                                    }
                                },
                                // Fallback for unsupported formats
                                _ => {
                                    println!("[AUDIO-WARN] Unsupported audio format: {:?}", audio_buf_ref.spec());
                                    
                                    // Simply initialize the samples to silence
                                    // This approach ensures we don't crash with unsupported formats
                                    // we'll later add support for more formats as needed
                                    for i in 0..s16_vec.len() {
                                        s16_vec[i] = 0;
                                    }
                                    
                                    // Log a more detailed warning about the specific audio format
                                    println!("[AUDIO-WARN] Falling back to silence for unsupported format. \n\tChannels: {}, \n\tRate: {}", 
                                             num_channels, 
                                             audio_buf_ref.spec().rate);
                                }
                            }

                            let frames_to_play = s16_vec.len() / num_channels;
                            let mut offset = 0;
                            while offset < frames_to_play {
                                match self.play_s16_buffer(&s16_vec[offset * num_channels..]) {
                                    Ok(frames_written) => {
                                        if frames_written > 0 {
                                            offset += frames_written;
                                        } else {
                                            println!("[AUDIO-DEBUG] ALSA wrote 0 frames, continuing loop.");
                                            break;
                                        }
                                    }
                                    Err(e @ AudioError::AlsaError(_)) => {
                                        println!("[AUDIO-ERROR] Error writing to ALSA: {}", e);
                                        pb.abandon_with_message(format!("ALSA Write Error: {}", e));
                                        return Err(e);
                                    }
                                    Err(e) => {
                                        pb.abandon_with_message(format!("Playback Error: {}", e));
                                        return Err(e);
                                    }
                                }
                            }
                        }
                        Err(SymphoniaError::DecodeError(err)) => {
                            println!("[AUDIO-WARN] Decode error: {}", err);
                        }
                        Err(e) => {
                            println!("[AUDIO-ERROR] Unexpected decoder error: {}", e);
                            pb.abandon_with_message(format!("Decoder Error: {}", e));
                            return Err(e.into());
                        }
                    }
                }
                Err(SymphoniaError::IoError(ref err)) if err.kind() == io::ErrorKind::UnexpectedEof => {
                    println!("[AUDIO-DEBUG] End of stream reached.");
                    pb.finish_with_message("Playback finished");
                    break 'decode_loop;
                }
                Err(SymphoniaError::ResetRequired) => {
                    println!("[AUDIO-WARN] Symphonia decoder reset required (unhandled).");
                    pb.abandon_with_message("Stream discontinuity (ResetRequired)");
                    break 'decode_loop;
                }
                Err(e) => {
                    println!("[AUDIO-ERROR] Error reading packet: {}", e);
                    // Check if it was our cancellation signal disguised as an IoError
                    if let SymphoniaError::IoError(ref io_err) = e {
                        if io_err.kind() == std::io::ErrorKind::Interrupted && io_err.to_string().contains("Playback cancelled") {
                             println!("[AUDIO-DEBUG] Confirmed playback cancellation.");
                             pb.abandon_with_message("Playback cancelled");
                             // Return Ok here, as cancellation isn't a "failure" in the traditional sense
                             // Or return a specific cancellation error if needed upstream
                             return Err(AudioError::StreamError("Playback cancelled".to_string()));
                        }
                    }
                    // Otherwise, it's a real error
                    pb.abandon_with_message(format!("Stream Read Error: {}", e));
                    return Err(e.into());
                }
            }
        }

        if let Some(pcm) = &self.pcm {
            pcm.drain()?;
        }

        Ok(())
    }

    /// Close the PCM device if it's open
    fn close_pcm(&mut self) {
        if let Some(pcm) = self.pcm.take() {
            let _ = pcm.drain();
            println!("[AUDIO-DEBUG] ALSA PCM closed.");
        }
    }

    /// Close the audio device and progress bar
    pub fn close(&mut self) {
        self.close_pcm();
        if let Some(pb) = self.progress_bar.take() {
            if !pb.is_finished() {
                pb.finish_and_clear();
            }
        }
    }
}

impl Drop for AlsaPlayer {
    fn drop(&mut self) {
        self.close();
    }
}

// --- Helper for Symphonia MediaSourceStream ---

struct ReqwestStreamWrapper {
    buffer: Vec<u8>,
    position: usize,
}

impl ReqwestStreamWrapper {
    // Create a new wrapper by downloading the entire stream first
    // This avoids needing to block on async code during Read operations
    async fn new_async(stream: impl futures_util::Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static) -> Result<Self, reqwest::Error> {
        use futures_util::StreamExt;
        
        let mut buffer = Vec::new();
        let mut stream = Box::pin(stream);
        
        // Pre-download all data to avoid async/sync issues
        while let Some(chunk_result) = stream.next().await {
            let chunk = chunk_result?;
            buffer.extend_from_slice(&chunk);
        }
        
        Ok(Self {
            buffer,
            position: 0,
        })
    }
}

impl io::Read for ReqwestStreamWrapper {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        // If we've reached the end of our buffer, we're done
        if self.position >= self.buffer.len() {
            return Ok(0);
        }
        
        // Calculate how many bytes we can read
        let available = self.buffer.len() - self.position;
        let to_read = std::cmp::min(buf.len(), available);
        
        // Copy from our internal buffer to the provided buffer
        buf[..to_read].copy_from_slice(&self.buffer[self.position..self.position + to_read]);
        self.position += to_read;
        
        Ok(to_read)
    }
}

impl io::Seek for ReqwestStreamWrapper {
    fn seek(&mut self, _pos: io::SeekFrom) -> io::Result<u64> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Seeking not supported on HTTP stream",
        ))
    }
}

impl MediaSource for ReqwestStreamWrapper {
     fn is_seekable(&self) -> bool {
         false
     }
     fn byte_len(&self) -> Option<u64> {
         None
     }
 }

unsafe impl Send for ReqwestStreamWrapper {}
unsafe impl Sync for ReqwestStreamWrapper {}
