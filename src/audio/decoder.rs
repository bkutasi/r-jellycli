use crate::audio::error::AudioError;
use tracing::{debug, error, info, trace, warn}; // Replaced log with tracing
use tracing::instrument;
use std::io;
use symphonia::core::audio::AudioBufferRef;
use symphonia::core::audio::{SignalSpec, AudioBuffer};
use symphonia::core::sample::i24; // Import the i24 type
use symphonia::core::codecs::{Decoder, DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::{FormatOptions, FormatReader, Packet};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use symphonia::core::units::TimeBase;
use tokio::sync::broadcast;
use tokio::task;

const LOG_TARGET: &str = "r_jellycli::audio::decoder";

/// Represents the outcome of trying to read the next packet.
// Removed #[derive(Debug)] because symphonia::core::formats::Packet doesn't implement Debug
enum PacketReadOutcome {
    Packet(Packet),
    EndOfStream,
    ShutdownSignalReceived,
}


/// Manages Symphonia format reading and decoding.
pub struct SymphoniaDecoder {
    format_reader: Option<Box<dyn FormatReader>>,
    decoder: Option<Box<dyn Decoder>>,
    track_id: u32,
    track_time_base: Option<TimeBase>,
    current_spec: Option<SignalSpec>, // Renamed from initial_spec
    sample_format: Option<symphonia::core::sample::SampleFormat>, // Added field
}


/// Represents the result of a decode operation returning a buffer reference.
/// Need to manage lifetime carefully, potentially requiring HRTBs or making the buffer owned.
/// For simplicity now, let's assume the caller handles the lifetime or we make it owned internally.
/// Let's try returning an owned buffer representing the ref for now.
pub enum DecodeRefResult {
    /// Successfully decoded audio data. Contains owned buffer and timestamp.

    // Decoded((AudioBufferRef<'?>, u64)), // Placeholder lifetime
    // Let's try returning an enum payload that owns the data
    DecodedOwned(DecodedBufferAndTimestamp),
    /// End of stream reached.
    EndOfStream,
    /// A recoverable error occurred (e.g., decode error, skip packet).
    Skipped(String),
    /// Shutdown signal received.
    Shutdown,
}

/// Holds an owned representation of a decoded buffer and its timestamp.
pub enum DecodedBufferAndTimestamp {
    U8(AudioBuffer<u8>, u64),
    S16(AudioBuffer<i16>, u64),
    S24(AudioBuffer<i24>, u64),
    S32(AudioBuffer<i32>, u64),
    F32(AudioBuffer<f32>, u64),
    F64(AudioBuffer<f64>, u64),
}


impl SymphoniaDecoder {
    /// Creates and initializes a new Symphonia decoder from a media source stream.
    #[instrument(skip(mss), target = LOG_TARGET)] // Instrument the constructor, skip complex mss
    pub fn new(mss: MediaSourceStream) -> Result<Self, AudioError> {
        debug!(target: LOG_TARGET, "Setting up Symphonia format reader and decoder...");
        let hint = Hint::new();
        let meta_opts: MetadataOptions = Default::default();
        let fmt_opts: FormatOptions = Default::default();

        // Use the passed-in mss for probing
        let probed = symphonia::default::get_probe().format(&hint, mss, &fmt_opts, &meta_opts)?;
        let format_reader = probed.format;

        // Find the default track.
        let track = format_reader
            .tracks()
            .iter()
            .find(|t| t.codec_params.codec != CODEC_TYPE_NULL) // Find first playable track
            .ok_or(AudioError::UnsupportedFormat("No suitable audio track found".to_string()))? // Find first playable track
            .clone();

        debug!(target: LOG_TARGET, "Found suitable audio track: ID={}, Codec={:?}", track.id, track.codec_params.codec);

        // Create a decoder for the track.
        let decoder_opts = DecoderOptions::default();
        let mut decoder = symphonia::default::get_codecs().make(&track.codec_params, &decoder_opts)?;

        // Reset the decoder state AFTER seeking the reader
        decoder.reset();
        debug!(target: LOG_TARGET, "Decoder reset after creation.");

        // Store the current signal specification (might be incomplete initially).
        let sample_rate = track.codec_params.sample_rate.ok_or(AudioError::MissingCodecParams("sample rate"))?;
        let channels = track.codec_params.channels; // Don't error if None yet
        let current_spec = channels.map(|chans| SignalSpec::new(sample_rate, chans));
        let sample_format = track.codec_params.sample_format;
        let time_base = track.codec_params.time_base; // Get time_base for logging

        debug!(target: LOG_TARGET, "Symphonia decoder created. Spec (potentially incomplete): {:?}, Sample Format: {:?}, TimeBase: {:?}", current_spec, sample_format, time_base);
        debug!(target: LOG_TARGET, "Detailed Params: Sample Rate = {:?}, Time Base = {:?}", track.codec_params.sample_rate, track.codec_params.time_base);

        Ok(Self {
            format_reader: Some(format_reader),
            decoder: Some(decoder),
            track_id: track.id,
            track_time_base: track.codec_params.time_base,
            current_spec, // Store the potentially incomplete spec
            sample_format,
        })
    }

/// Returns the sample format of the decoded audio track.
pub fn sample_format(&self) -> Option<symphonia::core::sample::SampleFormat> {
    self.sample_format
}

    /// Returns the current signal specification (might be updated after first decode).
    pub fn current_spec(&self) -> Option<SignalSpec> {
        self.current_spec
    }

    /// Returns the time base of the decoded track.
    pub fn time_base(&self) -> Option<TimeBase> {
        self.track_time_base
    }

    /// Reads the next packet from the format reader, handling blocking and shutdown.
    /// Takes ownership of reader/decoder and returns them.
    async fn read_next_packet(
        mut format_reader: Box<dyn FormatReader>,
        decoder: Box<dyn Decoder>, // Pass decoder through to maintain ownership chain (removed mut)
        shutdown_rx: &mut broadcast::Receiver<()>,
    ) -> Result<(Box<dyn FormatReader>, Box<dyn Decoder>, PacketReadOutcome), AudioError> {
        trace!(target: LOG_TARGET, "Entering read_next_packet");
        // Check for shutdown *before* potentially moving reader/decoder
        match shutdown_rx.try_recv() {
            Ok(_) => {
                info!(target: LOG_TARGET, "Shutdown signal received before reading packet.");
                // Return reader/decoder along with None packet to signal shutdown
                return Ok((format_reader, decoder, PacketReadOutcome::ShutdownSignalReceived));
            }
            Err(broadcast::error::TryRecvError::Empty) => {
                // No shutdown signal yet, proceed
            }
            Err(broadcast::error::TryRecvError::Lagged(_)) => {
                 warn!(target: LOG_TARGET, "Shutdown receiver lagged.");
                 // Treat as shutdown? Or proceed? Proceeding for now.
            }
            Err(broadcast::error::TryRecvError::Closed) => {
                 info!(target: LOG_TARGET, "Shutdown channel closed.");
                 // Return reader/decoder along with None packet to signal channel closed
                 return Ok((format_reader, decoder, PacketReadOutcome::ShutdownSignalReceived));
            }
        }

        trace!(target: LOG_TARGET, "read_next_packet: Creating blocking task future for next_packet...");
        // Create the future but don't await it yet
        let mut read_future = task::spawn_blocking(move || {
            // Move reader and decoder into the blocking task
            trace!(target: LOG_TARGET, "[SpawnBlocking] Calling format_reader.next_packet()...");
            let packet_res = format_reader.next_packet();
            // Add detailed logging of the result *before* returning from the closure
            match &packet_res {
                Ok(p) => trace!(target: LOG_TARGET, "[SpawnBlocking] format_reader.next_packet() returned Ok(Packet(ts={}, track={}))", p.ts(), p.track_id()),
                Err(SymphoniaError::IoError(ref io_err)) if io_err.kind() == std::io::ErrorKind::UnexpectedEof => {
                    trace!(target: LOG_TARGET, "[SpawnBlocking] format_reader.next_packet() returned Err(IoError(UnexpectedEof))");
                }
                Err(e) => {
                    trace!(target: LOG_TARGET, "[SpawnBlocking] format_reader.next_packet() returned Err: {:?}", e);
                }
            }
            // Return ownership along with the result
            (format_reader, decoder, packet_res)
        });

        trace!(target: LOG_TARGET, "read_next_packet: Selecting between read_future and shutdown_rx...");
        let blocking_result = tokio::select! {
            biased; // Prioritize shutdown check
            _ = shutdown_rx.recv() => {
                info!(target: LOG_TARGET, "Shutdown signal received during packet read wait.");
                // Abort the read future
                read_future.abort();
                // Indicate shutdown by returning a specific error. Ownership is lost.
                return Err(AudioError::ShutdownRequested);
            }
            join_result = &mut read_future => { // Poll by mutable reference
                trace!(target: LOG_TARGET, "read_next_packet: Read future completed.");
                join_result // This is Result<(Box<dyn FormatReader>, Box<dyn Decoder>, Result<Packet, SymphoniaError>), JoinError>
            }
        };
        trace!(target: LOG_TARGET, "read_next_packet: Blocking task completed.");

        trace!(target: LOG_TARGET, "read_next_packet: Spawn blocking result is_ok: {}", blocking_result.is_ok());
        match blocking_result {
            Ok((ret_reader, ret_decoder, Ok(packet))) => {
                // Return the reader/decoder received from the closure along with the packet
                Ok((ret_reader, ret_decoder, PacketReadOutcome::Packet(packet)))
            },
            Ok((ret_reader, ret_decoder, Err(e))) => { // Return reader/decoder even on error
                // Check if the error indicates end of stream
                if matches!(e, symphonia::core::errors::Error::IoError(ref io_err) if io_err.kind() == std::io::ErrorKind::UnexpectedEof) {
                     debug!(target: LOG_TARGET, "End of stream reached.");
                     // Return the reader/decoder received from the closure, with None packet
                     trace!(target: LOG_TARGET, "Returning Ok(EndOfStream) for EOF from read_next_packet");
                     Ok((ret_reader, ret_decoder, PacketReadOutcome::EndOfStream))
                } else {
                     // Log the specific error variant for debugging
                     error!(target: LOG_TARGET, "Error reading next packet (not EOF): {:?}", e);
                     // Propagate the error. Ownership was transferred, so reader/decoder state might be inconsistent.
                     // The caller must handle this potential inconsistency or loss of state.
                     // We cannot return the reader/decoder here as they are bound to the Ok variant.
                     trace!(target: LOG_TARGET, "Returning Err({:?}) from read_next_packet", e);
                     Err(AudioError::SymphoniaError(e))
                }
            },
            Err(join_error) => {
                 // Task failed, reader/decoder ownership is lost
                 error!(target: LOG_TARGET, "Spawn blocking task for next_packet failed: {}", join_error);
                 Err(AudioError::TaskJoinError(join_error.to_string()))
            }
        }
    }

    /// Decodes the next available audio frame, returning an owned buffer variant.
    /// Handles reading packets, decoding, and potential errors or stream end.
    #[instrument(skip(self, shutdown_rx), target = LOG_TARGET)] // Instrument the main decode function
    pub async fn decode_next_frame_owned(
        &mut self,
        shutdown_rx: &mut broadcast::Receiver<()>,
    ) -> Result<DecodeRefResult, AudioError> {
        // Take ownership of reader and decoder temporarily
        let mut format_reader = self.format_reader.take().ok_or(AudioError::InvalidState("Format reader not available".to_string()))?;
        let mut decoder = self.decoder.take().ok_or(AudioError::InvalidState("Decoder not available".to_string()))?;


        // Removed explicit re-seek calls from inside the decode loop.
        // Seeking should only happen upon explicit user action or initial setup if needed.

        loop {
            // --- Read Next Packet ---
            let read_result = Self::read_next_packet(format_reader, decoder, shutdown_rx).await;
            let packet_outcome: PacketReadOutcome;

            match read_result {
                // Success: Regain ownership of potentially updated reader/decoder
                Ok((new_format_reader, new_decoder, outcome)) => {
                    format_reader = new_format_reader; // Update local ownership with returned values
                    decoder = new_decoder;             // Update local ownership with returned values
                    packet_outcome = outcome;
                }
                // Error: Ownership is lost within read_next_packet. Propagate the error.
                // Do NOT try to restore self.format_reader/self.decoder here.
                Err(e) => {
                    // Check specifically for ShutdownRequested first
                    if matches!(e, AudioError::ShutdownRequested) {
                        info!(target: LOG_TARGET, "Shutdown requested during packet read. Decoder state lost.");
                        // Ownership was already lost in read_next_packet, ensure fields are None
                        self.format_reader = None;
                        self.decoder = None;
                        trace!(target: LOG_TARGET, "decode_next_frame_owned: Returning DecodeRefResult::Shutdown (from read_next_packet)");
                        return Ok(DecodeRefResult::Shutdown);
                    }

                    error!(target: LOG_TARGET, "Failed to read next packet, decoder state potentially lost: {}", e);
                    // Handle specific IO errors that indicate end-of-stream gracefully
                    if let AudioError::SymphoniaError(SymphoniaError::IoError(ref io_err)) = e {
                        match io_err.kind() {
                            io::ErrorKind::UnexpectedEof => {
                                info!(target: LOG_TARGET, "End of stream reached (UnexpectedEof).");
                                trace!(target: LOG_TARGET, "decode_next_frame: Returning DecodeResult::EndOfStream (UnexpectedEof)");
                                return Ok(DecodeRefResult::EndOfStream); // Use new enum
                            }
                            io::ErrorKind::Interrupted => {
                                info!(target: LOG_TARGET, "Playback interrupted (likely cancelled).");
                                trace!(target: LOG_TARGET, "decode_next_frame: Returning DecodeResult::Shutdown (Interrupted)");
                                return Ok(DecodeRefResult::Shutdown); // Use new enum
                            }
                            _ => {} // Fall through for other IO errors
                        }
                    }
                    // Handle other specific Symphonia errors
                    if let AudioError::SymphoniaError(SymphoniaError::ResetRequired) = e {
                        warn!(target: LOG_TARGET, "Symphonia decoder reset required (unhandled). Stopping.");
                        trace!(target: LOG_TARGET, "decode_next_frame: Returning DecodeResult::Skipped(\"Stream discontinuity (ResetRequired)\")");
                        return Ok(DecodeRefResult::Skipped("Stream discontinuity (ResetRequired)".to_string())); // Use new enum
                    }

                    // Handle generic errors - ownership already lost
                    error!(target: LOG_TARGET, "Unhandled error reading next packet: {}", e);
                    trace!(target: LOG_TARGET, "decode_next_frame: Returning Err({:?}) (Unhandled read error)", e);
                    return Err(e);
                }
            }

            // Handle the outcome from read_next_packet
            let packet = match packet_outcome {
                PacketReadOutcome::Packet(p) => p, // Got a packet, proceed to decode
                PacketReadOutcome::EndOfStream => {
                    info!(target: LOG_TARGET, "End of stream detected by read_next_packet.");
                    // Restore ownership before returning EndOfStream
                    self.format_reader = Some(format_reader);
                    self.decoder = Some(decoder);
                    trace!(target: LOG_TARGET, "decode_next_frame_owned: Returning DecodeRefResult::EndOfStream");
                    return Ok(DecodeRefResult::EndOfStream);
                }
                PacketReadOutcome::ShutdownSignalReceived => {
                    info!(target: LOG_TARGET, "Shutdown signal detected by read_next_packet.");
                    // Restore ownership before returning Shutdown
                    self.format_reader = Some(format_reader);
                    self.decoder = Some(decoder);
                    trace!(target: LOG_TARGET, "decode_next_frame_owned: Returning DecodeRefResult::Shutdown");
                    return Ok(DecodeRefResult::Shutdown);
                }
            };

            let packet_ts = packet.ts(); // Store timestamp before potential decode error
            // --- Process Packet ---
            if packet.track_id() != self.track_id {
                trace!(target: LOG_TARGET, "Skipping packet for track {}", packet.track_id());
                continue; // Read the next packet
            }

            // --- Decode Packet (in blocking task with shutdown check) ---
            // Clone packet data as it needs to move into the blocking task
            // Need to clone the packet itself, not just data, if decode needs metadata
            let packet_clone = packet.clone(); // Clone the whole packet
            let mut decode_future = task::spawn_blocking(move || {
                // Move decoder into the blocking task
                let decode_result = decoder.decode(&packet_clone);

                // Convert the result *inside* the blocking task
                let owned_result: Result<DecodedBufferAndTimestamp, SymphoniaError> = match decode_result {
                    Ok(audio_buf_ref) => {
                        // Convert BufferRef to Owned Enum Variant
                        match audio_buf_ref {
                            AudioBufferRef::U8(buf) => Ok(DecodedBufferAndTimestamp::U8(buf.into_owned(), packet_ts)),
                            AudioBufferRef::S16(buf) => Ok(DecodedBufferAndTimestamp::S16(buf.into_owned(), packet_ts)),
                            AudioBufferRef::S24(buf) => Ok(DecodedBufferAndTimestamp::S24(buf.into_owned(), packet_ts)),
                            AudioBufferRef::S32(buf) => Ok(DecodedBufferAndTimestamp::S32(buf.into_owned(), packet_ts)),
                            AudioBufferRef::F32(buf) => Ok(DecodedBufferAndTimestamp::F32(buf.into_owned(), packet_ts)),
                            AudioBufferRef::F64(buf) => Ok(DecodedBufferAndTimestamp::F64(buf.into_owned(), packet_ts)),
                            _ => Err(SymphoniaError::Unsupported("Unsupported AudioBufferRef variant")), // Return error for unsupported
                        }
                    }
                    Err(e) => Err(e), // Pass through Symphonia errors
                };

                // Return decoder ownership along with the *owned* result
                (decoder, owned_result)
            });

            // Select between shutdown and the decode future completing
            // Select between shutdown and the decode future completing
            // Poll decode_future by mutable reference (&mut) to retain ownership
            let decode_result_from_task: Result<DecodedBufferAndTimestamp, SymphoniaError> = tokio::select! {
                biased; // Prioritize shutdown check
                _ = shutdown_rx.recv() => {
                    info!(target: LOG_TARGET, "Shutdown signal received during decode wait.");
                    // Abort the decode future as we are shutting down (safe now as we poll by ref)
                    decode_future.abort();
                    // Restore format_reader ownership before returning
                    self.format_reader = Some(format_reader);
                    // Decoder ownership is lost as the task was aborted. Mark as None.
                    self.decoder = None; // Decoder is inside the aborted task
                    trace!(target: LOG_TARGET, "decode_next_frame_owned: Returning DecodeRefResult::Shutdown (during decode wait)");
                    return Ok(DecodeRefResult::Shutdown);
                }
                join_result = &mut decode_future => { // Poll by mutable reference
                    match join_result {
                        // Task completed, return the inner result (Ok(owned_buffer) or Err(SymphoniaError))
                        Ok((ret_decoder, owned_decode_res)) => {
                            decoder = ret_decoder; // Regain ownership of decoder
                            owned_decode_res // This is the Result<DecodedBufferAndTimestamp, SymphoniaError>
                        }
                        // Task panicked
                        Err(join_error) => {
                            error!(target: LOG_TARGET, "Spawn blocking task for decode failed: {}", join_error);
                            // Restore format_reader ownership, decoder is lost
                            self.format_reader = Some(format_reader);
                            self.decoder = None;
                            trace!(target: LOG_TARGET, "decode_next_frame_owned: Returning Err(TaskJoinError) (decode task panic)");
                            return Err(AudioError::TaskJoinError(join_error.to_string()));
                        }
                    }
                }
            };

            // --- Process Decode Result ---
            // --- Process Decode Result (which is now Result<DecodedBufferAndTimestamp, SymphoniaError>) ---
            match decode_result_from_task {
                 // Successfully decoded and converted to owned buffer
                 Ok(owned_buffer_ts) => {
                    // --- Update spec if needed ---
                    // Check if the current spec is missing channel info
                    if self.current_spec.map_or(true, |spec| spec.channels.count() == 0) {
                        // Extract spec from the decoded buffer
                        let decoded_spec = match &owned_buffer_ts {
                             DecodedBufferAndTimestamp::U8(buf, _) => buf.spec(),
                             DecodedBufferAndTimestamp::S16(buf, _) => buf.spec(),
                             DecodedBufferAndTimestamp::S24(buf, _) => buf.spec(),
                             DecodedBufferAndTimestamp::S32(buf, _) => buf.spec(),
                             DecodedBufferAndTimestamp::F32(buf, _) => buf.spec(),
                             DecodedBufferAndTimestamp::F64(buf, _) => buf.spec(),
                        };
                        // Update self.current_spec if the decoded spec has channels
                        if decoded_spec.channels.count() > 0 {
                             debug!(target: LOG_TARGET, "Updating decoder spec with info from first decoded frame: {:?}", decoded_spec);
                             self.current_spec = Some(*decoded_spec); // Dereference the spec
                        } else {
                             warn!(target: LOG_TARGET, "First decoded frame still missing channel info in spec: {:?}", decoded_spec);
                        }
                    }
                    // --- End Update spec ---

                    // Restore ownership before returning success
                    self.format_reader = Some(format_reader);
                    self.decoder = Some(decoder);

                    // Return the owned buffer variant wrapped in DecodeRefResult
                    trace!(target: LOG_TARGET, "decode_next_frame_owned: Returning DecodeRefResult::DecodedOwned(...)");
                    return Ok(DecodeRefResult::DecodedOwned(owned_buffer_ts));
                }
                // Specific Symphonia decode error (recoverable by skipping packet)
                Err(SymphoniaError::DecodeError(err)) => {
                    warn!(target: LOG_TARGET, "Symphonia decode error (skipping packet): {}", err);
                    // Continue loop to try next packet
                }
                 // Specific Symphonia unsupported error from conversion inside spawn_blocking
                 Err(SymphoniaError::Unsupported(err_str)) => {
                     error!(target: LOG_TARGET, "Unsupported format during decode/conversion: {}", err_str);
                     self.format_reader = Some(format_reader);
                     self.decoder = Some(decoder);
                     return Err(AudioError::UnsupportedFormat(err_str.to_string()));
                 }
                // Other Symphonia errors (treat as fatal for this operation)
                Err(e) => {
                    error!(target: LOG_TARGET, "Unexpected Symphonia error during decode: {}", e);
                    self.format_reader = Some(format_reader);
                    self.decoder = Some(decoder);
                    trace!(target: LOG_TARGET, "decode_next_frame: Returning Err({:?}) (Unexpected Symphonia error)", e);
                    return Err(e.into()); // Convert SymphoniaError to AudioError
                }
            }
        }
    }

    /// Resets the decoder state, potentially useful for seeking or error recovery.
    /// Note: This might discard internal decoder buffers.
    pub fn reset(&mut self) {
        if let Some(decoder) = self.decoder.as_mut() {
            decoder.reset();
            info!(target: LOG_TARGET, "Symphonia decoder reset.");
        }
        // Note: Resetting the format_reader might require re-probing or seeking,
        // which is complex and not implemented here. This reset is only for the decoder part.
    }

}