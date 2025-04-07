use crate::audio::{
    alsa_handler::AlsaPcmHandler, // Added
    alsa_writer::AlsaWriter,
    decoder::{DecodeRefResult, DecodedBufferAndTimestamp, SymphoniaDecoder},
    error::AudioError,
    processor::AudioProcessor,
    state_manager::PlaybackStateManager,
};
use rubato::{SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction}; // Added for resampler
use std::{any::TypeId, sync::{Arc, Mutex as StdMutex}, time::Duration}; // Added StdMutex
// use symphonia::core::audio::SignalSpec; // Removed - Unused import
use tokio::sync::{broadcast, Mutex as TokioMutex};
use tracing::{debug, error, info, trace, warn, instrument};

const LOG_TARGET: &str = "r_jellycli::audio::loop_runner";

/// Indicates the reason why the playback loop terminated successfully.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum PlaybackLoopExitReason {
    EndOfStream,
    ShutdownSignal,
}

// Helper extension trait for DecodedBufferAndTimestamp (copied from original playback.rs)
// Consider moving this to a shared utility module if used elsewhere.
trait DecodedBufferTimestampExt {
    fn type_id(&self) -> TypeId;
    fn timestamp(&self) -> u64;
    fn num_channels(&self) -> usize;
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

    fn num_channels(&self) -> usize {
         match self {
            DecodedBufferAndTimestamp::U8(buf, _) => buf.spec().channels.count(),
            DecodedBufferAndTimestamp::S16(buf, _) => buf.spec().channels.count(),
            DecodedBufferAndTimestamp::S24(buf, _) => buf.spec().channels.count(),
            DecodedBufferAndTimestamp::S32(buf, _) => buf.spec().channels.count(),
            DecodedBufferAndTimestamp::F32(buf, _) => buf.spec().channels.count(),
            DecodedBufferAndTimestamp::F64(buf, _) => buf.spec().channels.count(),
        }
    }
}


/// Runs the core playback loop, coordinating decoding, processing, state, and output.
pub struct PlaybackLoopRunner {
    decoder: SymphoniaDecoder,
    processor: Option<Arc<AudioProcessor>>, // Now optional, created after spec is known
    alsa_handler: Arc<StdMutex<AlsaPcmHandler>>, // Added direct handler access
    alsa_writer: Arc<AlsaWriter>,   // Still needed for writing/pausing etc.
    state_manager: Arc<TokioMutex<PlaybackStateManager>>,
    shutdown_rx: broadcast::Receiver<()>,
    alsa_initialized: bool, // Added flag
}

impl PlaybackLoopRunner {
    /// Creates a new PlaybackLoopRunner.
    pub fn new(
        decoder: SymphoniaDecoder,
        // processor: Arc<AudioProcessor>, // Removed - will be created internally
        alsa_handler: Arc<StdMutex<AlsaPcmHandler>>, // Added
        alsa_writer: Arc<AlsaWriter>,
        state_manager: Arc<TokioMutex<PlaybackStateManager>>,
        shutdown_rx: broadcast::Receiver<()>,
    ) -> Self {
        Self {
            decoder,
            processor: None, // Initialize as None
            alsa_handler,    // Store handler
            alsa_writer,
            state_manager,
            shutdown_rx,
            alsa_initialized: false, // Initialize flag
        }
    }

    /// Runs the main playback loop. Consumes the runner.
    #[instrument(skip(self), name = "playback_loop")]
    pub async fn run(mut self) -> Result<PlaybackLoopExitReason, AudioError> {
        info!(target: LOG_TARGET, "Starting playback loop.");
        let track_time_base = self.decoder.time_base();
        let mut was_paused = false; // Track previous pause state for transitions
        let mut current_decode_result: Option<Result<DecodeRefResult, AudioError>> = None; // Hold the result to be processed
        loop {
            // --- Pause Check and ALSA State Transition ---
            let is_paused = self.state_manager.lock().await.is_paused().await;
            trace!(target: LOG_TARGET, "Pause check: is_paused = {}, was_paused = {}", is_paused, was_paused);

            // --- Handle State Transitions ---
            if is_paused && !was_paused {
                // Transition: Playing -> Paused
                debug!(target: LOG_TARGET, "Pause state changed: Playing -> Paused. Requesting ALSA pause.");
                if let Err(e) = self.alsa_writer.pause_async().await {
                    warn!(target: LOG_TARGET, "Failed to pause ALSA device: {}. Continuing loop.", e);
                    // Decide if this is fatal? For now, log and continue.
                }
                was_paused = true; // Update state *after* attempting pause
            } else if !is_paused && was_paused {
                // Transition: Paused -> Playing
                debug!(target: LOG_TARGET, "Pause state changed: Paused -> Playing. Requesting ALSA resume.");
                 if let Err(e) = self.alsa_writer.resume_async().await {
                    warn!(target: LOG_TARGET, "Failed to resume ALSA device: {}. Continuing loop.", e);
                    // Log and continue.
                }
                was_paused = false; // Update state *after* attempting resume
            }
            // Note: was_paused is now up-to-date with the *intended* state.

            // --- Wait While Paused ---
            if is_paused {
                trace!(target: LOG_TARGET, "Entering wait loop (is_paused = true)");
                loop {
                    tokio::select! {
                        biased; // Prioritize shutdown check
                        _ = self.shutdown_rx.recv() => {
                            info!(target: LOG_TARGET, "Shutdown signal received while paused.");
                            debug!(target: LOG_TARGET, "Playback loop returning: Ok(ShutdownSignal) (from pause wait)");
                            return Ok(PlaybackLoopExitReason::ShutdownSignal);
                        }
                        // Check if pause state changed externally
                        maybe_changed = async {
                            tokio::time::sleep(Duration::from_millis(100)).await;
                            self.state_manager.lock().await.is_paused().await
                        } => {
                            if !maybe_changed {
                                debug!(target: LOG_TARGET, "Pause state changed: Paused -> Playing (detected in wait loop).");
                                // No need to call resume here, the outer loop logic will handle it on the next iteration
                                trace!(target: LOG_TARGET, "Exiting wait loop.");
                                break;
                            }
                        }
                    }
                }
                // After breaking the inner loop, continue to the top of the outer loop
                // where the Paused -> Playing transition will be handled.
                continue;
            }
            // --- End Pause Check ---

            trace!(target: LOG_TARGET, "--- Playback loop iteration start (after pause check) ---");

            // --- Decode Next Frame (Lookahead) ---
            let next_decode_result = self.decoder.decode_next_frame_owned(&mut self.shutdown_rx).await;
            trace!(target: LOG_TARGET, "Next decoder result: {:?}", next_decode_result.as_ref().map(|r| match r { DecodeRefResult::DecodedOwned(buf_ts) => format!("DecodedOwned(type={:?}, ts={})", buf_ts.type_id(), buf_ts.timestamp()), DecodeRefResult::EndOfStream => "EndOfStream".to_string(), DecodeRefResult::Skipped(s) => format!("Skipped({})", s), DecodeRefResult::Shutdown => "Shutdown".to_string() }).map_err(|e| format!("{:?}", e)));

            // --- Process the *current* decode result (from previous iteration) ---
            if let Some(decode_result_to_process) = current_decode_result.take() { // Take ownership
                trace!(target: LOG_TARGET, "Processing previous decode result...");

                match decode_result_to_process {
                Ok(DecodeRefResult::DecodedOwned(decoded_buffer_ts)) => {
                    // --- Deferred Initialization on First Successful Decode ---
                    if !self.alsa_initialized {
                        info!(target: LOG_TARGET, "First frame decoded. Initializing ALSA and Processor...");

                        // 1. Get Final Spec from Decoder
                        let final_spec = self.decoder.current_spec().ok_or_else(|| {
                            error!(target: LOG_TARGET, "Decoder spec still unavailable after first frame decode.");
                            AudioError::InitializationError("Decoder spec unavailable after first frame".to_string())
                        })?;
                        let num_channels = final_spec.channels.count();
                        if num_channels == 0 {
                             error!(target: LOG_TARGET, "Decoder spec has zero channels after first frame decode.");
                             return Err(AudioError::InitializationError("Decoder spec has zero channels".to_string()));
                        }
                        debug!(target: LOG_TARGET, "Obtained final decoder spec: {:?}", final_spec);

                        // 2. Initialize ALSA Handler
                        let actual_rate = {
                            let mut handler_guard = self.alsa_handler.lock().map_err(|e| {
                                error!(target: LOG_TARGET, "ALSA handler mutex poisoned on deferred init: {}", e);
                                AudioError::InvalidState(format!("ALSA handler mutex poisoned on deferred init: {}", e))
                            })?;
                            handler_guard.initialize(final_spec)?;
                            handler_guard.get_actual_rate().ok_or_else(|| {
                                error!(target: LOG_TARGET, "ALSA handler did not return actual rate after deferred initialization.");
                                AudioError::InitializationError("ALSA handler missing actual rate after deferred init".to_string())
                            })?
                        };
                        debug!(target: LOG_TARGET, "ALSA initialized. Decoder rate: {}, ALSA actual rate: {}", final_spec.rate, actual_rate);

                        // 3. Setup Resampler (Conditional)
                        let decoder_rate = final_spec.rate;
                        let resampler_instance: Option<Arc<TokioMutex<SincFixedIn<f32>>>> = if decoder_rate != actual_rate {
                            info!(target: LOG_TARGET, "Sample rate mismatch (Decoder: {}, ALSA: {}). Initializing resampler.", decoder_rate, actual_rate);
                            let params = SincInterpolationParameters {
                                sinc_len: 256, f_cutoff: 0.95, interpolation: SincInterpolationType::Linear,
                                oversampling_factor: 256, window: WindowFunction::BlackmanHarris2,
                            };
                            let chunk_size = 512; // TODO: Make configurable or derive?
                            let resampler = SincFixedIn::<f32>::new(
                                actual_rate as f64 / decoder_rate as f64, 2.0, params, chunk_size, num_channels,
                            ).map_err(|e| {
                                error!(target: LOG_TARGET, "Failed to create resampler: {}", e);
                                AudioError::InitializationError(format!("Failed to create resampler: {}", e))
                            })?;
                            Some(Arc::new(TokioMutex::new(resampler)))
                        } else {
                            info!(target: LOG_TARGET, "Sample rates match ({} Hz). Resampling disabled.", actual_rate);
                            None
                        };

                        // 4. Create Audio Processor
                        self.processor = Some(Arc::new(AudioProcessor::new(resampler_instance, num_channels)));
                        info!(target: LOG_TARGET, "AudioProcessor created.");

                        // 5. Mark as Initialized
                        self.alsa_initialized = true;
                        info!(target: LOG_TARGET, "Deferred initialization complete.");
                    }
                    // --- End Deferred Initialization ---

                    // first_decode_done = true; // No longer needed
                    let current_ts = decoded_buffer_ts.timestamp();
                    let num_channels = decoded_buffer_ts.num_channels(); // Get num_channels before moving

                    // --- Process Buffer (Resample/Convert) ---
                    // Process based on the dynamic type within DecodedBufferAndTimestamp
                    // --- Process Buffer (Resample/Convert) ---
                    // Use the processor created during deferred initialization
                    let processor = self.processor.as_ref().ok_or_else(|| {
                         // This should not happen if alsa_initialized is true
                         error!(target: LOG_TARGET, "Processor accessed before initialization after first frame decode.");
                         AudioError::InvalidState("Processor accessed before initialization".to_string())
                    })?;
                    // Determine if this is the last buffer based on the *next* decode result
                    let is_last = matches!(next_decode_result, Ok(DecodeRefResult::EndOfStream) | Ok(DecodeRefResult::Shutdown) | Err(_));
                    trace!(target: LOG_TARGET, "Calling process_buffer with is_last = {}", is_last);

                    let processing_result = match decoded_buffer_ts {
                         // Pass is_last flag
                        DecodedBufferAndTimestamp::U8(audio_buffer, _) => processor.process_buffer(audio_buffer, is_last).await,
                        DecodedBufferAndTimestamp::S16(audio_buffer, _) => processor.process_buffer(audio_buffer, is_last).await,
                        DecodedBufferAndTimestamp::S24(audio_buffer, _) => processor.process_buffer(audio_buffer, is_last).await,
                        DecodedBufferAndTimestamp::S32(audio_buffer, _) => processor.process_buffer(audio_buffer, is_last).await,
                        DecodedBufferAndTimestamp::F32(audio_buffer, _) => processor.process_buffer(audio_buffer, is_last).await,
                        DecodedBufferAndTimestamp::F64(audio_buffer, _) => processor.process_buffer(audio_buffer, is_last).await,
                    };

                    // --- Update Progress ---
                    // Update regardless of processing result, using the original timestamp
                    trace!(target: LOG_TARGET, "Updating progress with TS: {}, TimeBase: {:?}", current_ts, track_time_base); // Log TS and TimeBase
                    debug!(target: LOG_TARGET, "Passing to update_progress: current_ts = {}, track_time_base = {:?}", current_ts, track_time_base);
                    self.state_manager.lock().await.update_progress(current_ts, track_time_base).await;

                    // --- Handle Processing Result ---
                    match processing_result {
                        Ok(Some(s16_vec)) => {
                            // --- ALSA Playback ---
                            trace!(target: LOG_TARGET, "Calling write_s16_buffer_async with {} interleaved samples...", s16_vec.len());
                            if let Err(e) = self.alsa_writer.write_s16_buffer_async(s16_vec, num_channels, &mut self.shutdown_rx).await {
                                if matches!(e, AudioError::ShutdownRequested) {
                                    info!(target: LOG_TARGET, "Shutdown requested during ALSA write, exiting playback loop.");
                                    debug!(target: LOG_TARGET, "Playback loop returning: Ok(ShutdownSignal) (from ALSA write)");
                                    return Ok(PlaybackLoopExitReason::ShutdownSignal);
                                }
                                error!(target: LOG_TARGET, "ALSA Write Error: {}", e);
                                debug!(target: LOG_TARGET, "Playback loop returning: Err({:?}) (from ALSA write)", e);
                                return Err(e);
                            }
                        }
                        Ok(None) => {
                            trace!(target: LOG_TARGET, "Processor returned None (skipped buffer), continuing loop.");
                            continue;
                        }
                        Err(e) => {
                            error!(target: LOG_TARGET, "Audio Processor Error: {}", e);
                            debug!(target: LOG_TARGET, "Playback loop returning: Err({:?}) (from processor)", e);
                            return Err(e);
                        }
                    }
                } // End Ok(DecodeRefResult::DecodedOwned)
                Ok(DecodeRefResult::Skipped(reason)) => {
                    warn!(target: LOG_TARGET, "Decoder skipped packet (in processed result): {}", reason);
                    // No buffer to process, just continue to next decode
                }
                // EndOfStream and Shutdown from the *processed* result shouldn't happen here
                // because we process *before* checking the next result for EOS/Shutdown.
                // If they occur, it implies an issue with the state logic.
                Ok(DecodeRefResult::EndOfStream) => {
                     error!(target: LOG_TARGET, "Logical error: Encountered EndOfStream in the buffer being processed. Should have been handled by lookahead.");
                     // Treat as EOS for safety, but log error
                     debug!(target: LOG_TARGET, "Playback loop returning: Ok(EndOfStream) (unexpected location)");
                     return Ok(PlaybackLoopExitReason::EndOfStream);
                }
                Ok(DecodeRefResult::Shutdown) => {
                     error!(target: LOG_TARGET, "Logical error: Encountered Shutdown in the buffer being processed. Should have been handled by lookahead.");
                     // Treat as Shutdown for safety
                     debug!(target: LOG_TARGET, "Playback loop returning: Ok(ShutdownSignal) (unexpected location)");
                     return Ok(PlaybackLoopExitReason::ShutdownSignal);
                }
                Err(e) => {
                    // This was an error from the *previous* decode attempt
                    error!(target: LOG_TARGET, "Fatal decoder error (from previous iteration): {}", e);
                    debug!(target: LOG_TARGET, "Playback loop returning: Err({:?}) (from previous fatal decoder error)", e);
                    return Err(e);
                }
            } // End if let Some(decode_result_to_process)
            }

            // --- Handle the *next* decode result (determining loop continuation/exit) ---
            match next_decode_result {
                Ok(DecodeRefResult::DecodedOwned(decoded_buffer_ts)) => {
                    // Store for processing in the *next* iteration
                    current_decode_result = Some(Ok(DecodeRefResult::DecodedOwned(decoded_buffer_ts)));
                    trace!(target: LOG_TARGET, "Stored next buffer for processing in next iteration.");
                    // Continue the loop
                }
                Ok(DecodeRefResult::Skipped(reason)) => {
                    warn!(target: LOG_TARGET, "Decoder skipped packet (next result): {}", reason);
                    // Store the skip result to be handled (logged) in the next iteration's processing block
                    current_decode_result = Some(Ok(DecodeRefResult::Skipped(reason)));
                    // Continue the loop
                }
                Ok(DecodeRefResult::EndOfStream) => {
                    // The last buffer (if any) was just processed with is_last=true.
                    // Now we can safely exit.
                    info!(target: LOG_TARGET, "Decoder reached end of stream (next result). Exiting loop.");
                    // Check if ALSA was ever initialized. If not, it means we hit EOF before getting a valid frame.
                    if !self.alsa_initialized {
                        error!(target: LOG_TARGET, "Decoder reached end of stream before first valid frame. Stream might be empty or invalid.");
                        debug!(target: LOG_TARGET, "Playback loop returning: Err(EmptyStream) (EOF before init)");
                        return Err(AudioError::EmptyStream);
                    }
                    debug!(target: LOG_TARGET, "Playback loop returning: Ok(EndOfStream)");
                    return Ok(PlaybackLoopExitReason::EndOfStream);
                }
                 Ok(DecodeRefResult::Shutdown) => {
                    // The last buffer (if any) was just processed with is_last=true.
                    // Now we can safely exit due to shutdown.
                    info!(target: LOG_TARGET, "Decoder indicated shutdown (next result). Exiting loop.");
                    debug!(target: LOG_TARGET, "Playback loop returning: Ok(ShutdownSignal) (from decoder signal)");
                    return Ok(PlaybackLoopExitReason::ShutdownSignal);
                }
                Err(e) => {
                    // Store the error to be returned in the next iteration's processing block
                    error!(target: LOG_TARGET, "Fatal decoder error (next result): {}", e);
                    current_decode_result = Some(Err(e));
                    // Let the loop run one more time to handle the error return inside the processing block
                }
            }
        }
    }
}