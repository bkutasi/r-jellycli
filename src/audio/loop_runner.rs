use crate::audio::{
    alsa_writer::AlsaWriter,
    decoder::{DecodeRefResult, DecodedBufferAndTimestamp, SymphoniaDecoder},
    error::AudioError,
    processor::AudioProcessor,
    state_manager::PlaybackStateManager,
};
use std::{any::TypeId, sync::Arc, time::Duration};
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
    processor: Arc<AudioProcessor>, // Use Arc for shared access if needed, or owned if exclusive
    alsa_writer: Arc<AlsaWriter>,   // Use Arc for shared access
    state_manager: Arc<TokioMutex<PlaybackStateManager>>, // Use Arc<Mutex> for interior mutability
    shutdown_rx: broadcast::Receiver<()>,
}

impl PlaybackLoopRunner {
    /// Creates a new PlaybackLoopRunner.
    pub fn new(
        decoder: SymphoniaDecoder,
        processor: Arc<AudioProcessor>,
        alsa_writer: Arc<AlsaWriter>,
        state_manager: Arc<TokioMutex<PlaybackStateManager>>,
        shutdown_rx: broadcast::Receiver<()>,
    ) -> Self {
        Self {
            decoder,
            processor,
            alsa_writer,
            state_manager,
            shutdown_rx,
        }
    }

    /// Runs the main playback loop. Consumes the runner.
    #[instrument(skip(self), name = "playback_loop")]
    pub async fn run(mut self) -> Result<PlaybackLoopExitReason, AudioError> {
        info!(target: LOG_TARGET, "Starting playback loop.");
        let track_time_base = self.decoder.time_base();
        let mut was_paused = false; // Track previous pause state for transitions
        let mut first_decode_done = false; // Track if we've successfully decoded at least one frame
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

            // --- Decode Next Frame ---
            let decode_result = self.decoder.decode_next_frame_owned(&mut self.shutdown_rx).await;
            trace!(target: LOG_TARGET, "Decoder result: {:?}", decode_result.as_ref().map(|r| match r { DecodeRefResult::DecodedOwned(buf_ts) => format!("DecodedOwned(type={:?}, ts={})", buf_ts.type_id(), buf_ts.timestamp()), DecodeRefResult::EndOfStream => "EndOfStream".to_string(), DecodeRefResult::Skipped(s) => format!("Skipped({})", s), DecodeRefResult::Shutdown => "Shutdown".to_string() }).map_err(|e| format!("{:?}", e)));

            match decode_result {
                Ok(DecodeRefResult::DecodedOwned(decoded_buffer_ts)) => {
                    first_decode_done = true; // Mark first successful decode
                    let current_ts = decoded_buffer_ts.timestamp();
                    let num_channels = decoded_buffer_ts.num_channels(); // Get num_channels before moving

                    // --- Process Buffer (Resample/Convert) ---
                    // Process based on the dynamic type within DecodedBufferAndTimestamp
                    let processing_result = match decoded_buffer_ts {
                        DecodedBufferAndTimestamp::U8(audio_buffer, _) => self.processor.process_buffer(audio_buffer).await,
                        DecodedBufferAndTimestamp::S16(audio_buffer, _) => self.processor.process_buffer(audio_buffer).await,
                        DecodedBufferAndTimestamp::S24(audio_buffer, _) => self.processor.process_buffer(audio_buffer).await,
                        DecodedBufferAndTimestamp::S32(audio_buffer, _) => self.processor.process_buffer(audio_buffer).await,
                        DecodedBufferAndTimestamp::F32(audio_buffer, _) => self.processor.process_buffer(audio_buffer).await,
                        DecodedBufferAndTimestamp::F64(audio_buffer, _) => self.processor.process_buffer(audio_buffer).await,
                    };

                    // --- Update Progress ---
                    // Update regardless of processing result, using the original timestamp
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
                }
                Ok(DecodeRefResult::Skipped(reason)) => {
                     warn!(target: LOG_TARGET, "Decoder skipped packet: {}", reason);
                     first_decode_done = true; // Mark first decode attempt as done even if skipped
                     continue;
                }
                Ok(DecodeRefResult::EndOfStream) => {
                    if !first_decode_done {
                        error!(target: LOG_TARGET, "Decoder reached end of stream immediately. Stream might be empty or invalid.");
                        debug!(target: LOG_TARGET, "Playback loop returning: Err(EmptyStream) (immediate EOF)");
                        return Err(AudioError::EmptyStream); // Return specific error for immediate EOF
                    }
                    info!(target: LOG_TARGET, "Decoder reached end of stream after processing data. Flushing processor...");
                    // --- Flush Processor ---
                    match self.processor.flush_resampler().await {
                        Ok(Some(s16_vec)) => {
                            if !s16_vec.is_empty() {
                                // Need num_channels for the flushed buffer. Assume it's the same as the processor was configured with.
                                // This might be fragile if the processor doesn't store it. Let's assume AudioProcessor stores num_channels.
                                // We need to get num_channels from the processor or the original spec.
                                // Let's modify AudioProcessor to store num_channels if it doesn't already.
                                // Assuming AudioProcessor has a method `output_channels()` or similar.
                                // For now, let's try getting it from the decoder's initial spec if possible,
                                // otherwise fall back to a default (e.g., 2). This is not ideal.
                                // *** Refinement Needed: Ensure channel count is reliably available for flushed buffer ***
                                let num_channels = self.decoder.initial_spec().map_or(2, |s| s.channels.count());
                                info!(target: LOG_TARGET, "Writing flushed buffer ({} samples, {} channels) to ALSA...", s16_vec.len(), num_channels);
                                if let Err(e) = self.alsa_writer.write_s16_buffer_async(s16_vec, num_channels, &mut self.shutdown_rx).await {
                                     if matches!(e, AudioError::ShutdownRequested) {
                                        info!(target: LOG_TARGET, "Shutdown requested during final ALSA write, exiting loop.");
                                        debug!(target: LOG_TARGET, "Playback loop returning: Ok(ShutdownSignal) (from final ALSA write)");
                                        return Ok(PlaybackLoopExitReason::ShutdownSignal);
                                    }
                                    error!(target: LOG_TARGET, "Error writing flushed buffer to ALSA: {}", e);
                                    debug!(target: LOG_TARGET, "Playback loop returning: Err({:?}) (from final ALSA write)", e);
                                    return Err(e);
                                }
                            } else {
                                trace!(target: LOG_TARGET, "Processor flushed empty buffer, skipping final write.");
                            }
                        }
                        Ok(None) => {
                            trace!(target: LOG_TARGET, "Processor flush returned no data.");
                        }
                        Err(e) => {
                            error!(target: LOG_TARGET, "Error flushing processor: {}", e);
                            debug!(target: LOG_TARGET, "Playback loop returning: Err({:?}) (from processor flush)", e);
                            return Err(e);
                        }
                    }
                    // --- Proceed with normal EndOfStream after flushing ---
                    info!(target: LOG_TARGET, "Finished flushing. Breaking playback loop due to EndOfStream.");
                    debug!(target: LOG_TARGET, "Playback loop returning: Ok(EndOfStream)");
                    return Ok(PlaybackLoopExitReason::EndOfStream);
                }
                Ok(DecodeRefResult::Shutdown) => {
                    // If shutdown is detected *here* (meaning it happened during decode_next_frame_owned),
                    // we should exit immediately without flushing.
                    info!(target: LOG_TARGET, "Decoder indicated shutdown during decode. Exiting loop without flushing.");
                    debug!(target: LOG_TARGET, "Playback loop returning: Ok(ShutdownSignal) (from decoder signal)");
                    return Ok(PlaybackLoopExitReason::ShutdownSignal); // Return directly
                }
                Err(e) => {
                    error!(target: LOG_TARGET, "Fatal decoder error: {}", e);
                    debug!(target: LOG_TARGET, "Playback loop returning: Err({:?}) (from fatal decoder error)", e);
                    return Err(e);
                }
            }
        }
    }
}