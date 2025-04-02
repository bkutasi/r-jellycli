use crate::audio::error::AudioError;
use log::{debug, error, info, trace, warn};
use std::io;
use symphonia::core::audio::AudioBufferRef;
// Removed unused Signal import
use symphonia::core::audio::{SignalSpec, AudioBuffer}; // Removed unused AudioBufferRef
use symphonia::core::sample::i24; // Import the i24 type
use symphonia::core::codecs::{Decoder, DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::{FormatOptions, FormatReader, Packet};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use symphonia::core::units::TimeBase;
// Removed unused TypeId import
use tokio::sync::broadcast;
use tokio::task; // Restore task import

const LOG_TARGET: &str = "r_jellycli::audio::decoder";

/// Manages Symphonia format reading and decoding.
pub struct SymphoniaDecoder {
    format_reader: Option<Box<dyn FormatReader>>,
    decoder: Option<Box<dyn Decoder>>,
    track_id: u32,
    track_time_base: Option<TimeBase>,
    initial_spec: Option<SignalSpec>,
    sample_format: Option<symphonia::core::sample::SampleFormat>, // Added field
}

// Removed old DecodeResult enum

/// Represents the result of a decode operation returning a buffer reference.
/// Need to manage lifetime carefully, potentially requiring HRTBs or making the buffer owned.
/// For simplicity now, let's assume the caller handles the lifetime or we make it owned internally.
/// Let's try returning an owned buffer representing the ref for now.
pub enum DecodeRefResult {
    /// Successfully decoded audio data. Contains owned buffer and timestamp.
    // Removed problematic Decoded variant as Sample is not object-safe
    // Let's try returning the AudioBufferRef directly and deal with lifetimes later if needed.
    // Decoded((AudioBufferRef<'static>, u64)), // This lifetime is likely wrong.
    // Let's stick to the original plan: return AudioBufferRef and let playback.rs handle conversion.
    // We need a lifetime parameter.
    // pub enum DecodeRefResult<'a> { Decoded((AudioBufferRef<'a>, u64)), ... }
    // This complicates the async function signature.
    // Alternative: Return an enum that holds different owned buffer types?
    // Let's try modifying DecodeResult itself for now to hold AudioBufferRef temporarily.
    // This is messy. Let's create DecodeRefResult but return owned data for now.
    // How about returning the raw packet data? No, that defeats the purpose.
    // Let's go back to the idea of decode_next_ref returning AudioBufferRef.
    // The lifetime issue is tricky in async.
    // Maybe the simplest is to modify decode_next_frame to return Result<Option<(AudioBufferRef<'decoder_lifetime>, u64)>, AudioError>
    // where 'decoder_lifetime is the lifetime of the decoder instance?
    // Let's define DecodeRefResult without lifetimes for now and see where it breaks.

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
    S24(AudioBuffer<i24>, u64), // Use the correct i24 type
    S32(AudioBuffer<i32>, u64),
    F32(AudioBuffer<f32>, u64),
    F64(AudioBuffer<f64>, u64),
}


impl SymphoniaDecoder {
    /// Creates and initializes a new Symphonia decoder from a media source stream.
    pub fn new(mss: MediaSourceStream) -> Result<Self, AudioError> { // Restore signature
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
            .ok_or(AudioError::UnsupportedFormat("No suitable audio track found".to_string()))?
            .clone(); // Clone track info

        debug!(target: LOG_TARGET, "Found suitable audio track: ID={}, Codec={:?}", track.id, track.codec_params.codec);

        // Create a decoder for the track.
        let decoder_opts = DecoderOptions::default();
        let mut decoder = symphonia::default::get_codecs().make(&track.codec_params, &decoder_opts)?;

        // Reset the decoder state AFTER seeking the reader
        decoder.reset();
        debug!(target: LOG_TARGET, "Decoder reset after creation.");

        // Store the initial signal specification.
        let initial_spec = SignalSpec::new(
            track.codec_params.sample_rate.ok_or(AudioError::MissingCodecParams("sample rate"))?,
            track.codec_params.channels.ok_or(AudioError::MissingCodecParams("channels map"))?,
        );
        // Store the sample format
        let sample_format = track.codec_params.sample_format;

        debug!(target: LOG_TARGET, "Symphonia decoder created successfully. Initial Spec: {:?}, Sample Format: {:?}", initial_spec, sample_format);

        Ok(Self {
            format_reader: Some(format_reader),
            decoder: Some(decoder),
            track_id: track.id,
            track_time_base: track.codec_params.time_base,
            initial_spec: Some(initial_spec), // Correct indentation
            sample_format, // Correct indentation
        }) // End Ok(Self { ... })
    }

/// Returns the sample format of the decoded audio track.
pub fn sample_format(&self) -> Option<symphonia::core::sample::SampleFormat> {
    self.sample_format
}

    /// Returns the initial signal specification detected.
    pub fn initial_spec(&self) -> Option<SignalSpec> {
        self.initial_spec
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
    // Restore original return type including reader/decoder
    ) -> Result<(Box<dyn FormatReader>, Box<dyn Decoder>, Option<Packet>), AudioError> {
        trace!(target: LOG_TARGET, "Entering read_next_packet");
        // Check for shutdown *before* potentially moving reader/decoder
        match shutdown_rx.try_recv() {
            Ok(_) => {
                info!(target: LOG_TARGET, "Shutdown signal received before reading packet.");
                // Return reader/decoder along with None packet to signal shutdown
                return Ok((format_reader, decoder, None));
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
                 return Ok((format_reader, decoder, None));
            }
        }

        // Restore spawn_blocking for the synchronous next_packet call
        trace!(target: LOG_TARGET, "read_next_packet: Spawning blocking task for next_packet...");
        let blocking_result = task::spawn_blocking(move || {
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
        }).await;
        trace!(target: LOG_TARGET, "read_next_packet: Blocking task completed.");

        trace!(target: LOG_TARGET, "read_next_packet: Spawn blocking result is_ok: {}", blocking_result.is_ok());
        match blocking_result {
            Ok((ret_reader, ret_decoder, Ok(packet))) => {
                // trace!(target: LOG_TARGET, "Read packet: track={}, ts={}, duration={}", packet.track_id(), packet.ts(), packet.dur());
                // Return the reader/decoder received from the closure along with the packet
                Ok((ret_reader, ret_decoder, Some(packet)))
            },
            Ok((ret_reader, ret_decoder, Err(e))) => { // Return reader/decoder even on error
                // Check if the error indicates end of stream
                if matches!(e, symphonia::core::errors::Error::IoError(ref io_err) if io_err.kind() == std::io::ErrorKind::UnexpectedEof) {
                     debug!(target: LOG_TARGET, "End of stream reached.");
                     // Return the reader/decoder received from the closure, with None packet
                     trace!(target: LOG_TARGET, "Returning Ok(None) for EOF from read_next_packet");
                     Ok((ret_reader, ret_decoder, None))
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
            Err(join_error) => { // Restore JoinError handling
                 // Task failed, reader/decoder ownership is lost
                 error!(target: LOG_TARGET, "Spawn blocking task for next_packet failed: {}", join_error);
                 Err(AudioError::TaskJoinError(join_error.to_string()))
            }
        } // End match blocking_result
    } // End fn read_next_packet

    /// Decodes the next available audio frame, returning an owned buffer variant.
    /// Handles reading packets, decoding, and potential errors or stream end.
    pub async fn decode_next_frame_owned( // Renamed function
        &mut self,
        shutdown_rx: &mut broadcast::Receiver<()>,
    ) -> Result<DecodeRefResult, AudioError> { // Return new enum type
        // Take ownership of reader and decoder temporarily
        let mut format_reader = self.format_reader.take().ok_or(AudioError::InvalidState("Format reader not available".to_string()))?;
        let mut decoder = self.decoder.take().ok_or(AudioError::InvalidState("Decoder not available".to_string()))?;


        // Removed explicit re-seek calls from inside the decode loop.
        // Seeking should only happen upon explicit user action or initial setup if needed.

        loop {
            // --- Read Next Packet ---
            let read_result = Self::read_next_packet(format_reader, decoder, shutdown_rx).await;
            let packet_opt: Option<Packet>;

            match read_result {
                // Success: Regain ownership of potentially updated reader/decoder
                Ok((new_format_reader, new_decoder, opt_packet)) => {
                    format_reader = new_format_reader; // Update local ownership with returned values
                    decoder = new_decoder;             // Update local ownership with returned values
                    packet_opt = opt_packet;
                }
                 // Error: Ownership is lost within read_next_packet. Propagate the error.
                 // Do NOT try to restore self.format_reader/self.decoder here.
                Err(e) => {
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

            // Check if shutdown occurred during read or if packet is None (EOF)
            let packet = match packet_opt {
                Some(p) => p,

                None => {
                    info!(target: LOG_TARGET, "No more packets or shutdown signal received.");
                    // Restore ownership before returning EndOfStream or Shutdown
                    self.format_reader = Some(format_reader);
                    self.decoder = Some(decoder);
                    // Determine if it was EOF or shutdown
                    // We check the receiver again, though it might have been consumed already.
                    // A more robust way might involve passing the shutdown status explicitly.
                    // Determine if it was EOF or shutdown based on shutdown_rx state and return directly
                    return match shutdown_rx.try_recv() {
                        Err(broadcast::error::TryRecvError::Empty) => {
                            trace!(target: LOG_TARGET, "decode_next_frame_owned: Returning DecodeRefResult::EndOfStream (No packet/EOF)");
                            Ok(DecodeRefResult::EndOfStream)
                        },
                        _ => { // Covers Ok(_), Err(Lagged), Err(Closed) -> treat as shutdown
                            trace!(target: LOG_TARGET, "decode_next_frame_owned: Returning DecodeRefResult::Shutdown (Signal received or channel closed)");
                            Ok(DecodeRefResult::Shutdown)
                        }
                    };
                }
            };

            let packet_ts = packet.ts(); // Store timestamp before potential decode error
            // --- Process Packet ---
            if packet.track_id() != self.track_id {
                trace!(target: LOG_TARGET, "Skipping packet for track {}", packet.track_id());
                continue; // Read the next packet
            }

            match decoder.decode(&packet) {
                Ok(audio_buf_ref) => {
                    // --- Check for Spec Changes ---
                    if let Some(initial_spec) = self.initial_spec {
                         if audio_buf_ref.spec() != &initial_spec {
                            warn!(
                                target: LOG_TARGET,
                                "Audio specification changed mid-stream! Expected: {:?}, Got: {:?}. Stopping.",
                                initial_spec, audio_buf_ref.spec()
                            );
                            self.format_reader = Some(format_reader);
                            self.decoder = Some(decoder);
                            trace!(target: LOG_TARGET, "decode_next_frame: Returning Err(AudioError::UnsupportedFormat(\"Dynamic spec change\"))");
                            return Err(AudioError::UnsupportedFormat("Dynamic spec change".to_string()));
                        }
                    } else {
                         error!(target: LOG_TARGET, "Initial spec missing during decode check!");
                         self.format_reader = Some(format_reader);
                         self.decoder = Some(decoder);
                         trace!(target: LOG_TARGET, "decode_next_frame: Returning Err(AudioError::InvalidState(\"Initial spec missing\"))");
                         return Err(AudioError::InvalidState("Initial spec missing".to_string()));
                    }

                    // --- Convert BufferRef to Owned Enum Variant ---
                    let owned_decoded_buffer = match audio_buf_ref {
                        // Use .into_owned() to convert Cow -> owned AudioBuffer
                        AudioBufferRef::U8(buf) => DecodeRefResult::DecodedOwned(DecodedBufferAndTimestamp::U8(buf.into_owned(), packet_ts)),
                        AudioBufferRef::S16(buf) => DecodeRefResult::DecodedOwned(DecodedBufferAndTimestamp::S16(buf.into_owned(), packet_ts)),
                        AudioBufferRef::S24(buf) => DecodeRefResult::DecodedOwned(DecodedBufferAndTimestamp::S24(buf.into_owned(), packet_ts)),
                        AudioBufferRef::S32(buf) => DecodeRefResult::DecodedOwned(DecodedBufferAndTimestamp::S32(buf.into_owned(), packet_ts)),
                        AudioBufferRef::F32(buf) => DecodeRefResult::DecodedOwned(DecodedBufferAndTimestamp::F32(buf.into_owned(), packet_ts)),
                        AudioBufferRef::F64(buf) => DecodeRefResult::DecodedOwned(DecodedBufferAndTimestamp::F64(buf.into_owned(), packet_ts)),
                        _ => {
                             error!(target: LOG_TARGET, "Unsupported AudioBufferRef variant encountered during decode.");
                             // Restore ownership before returning error
                             self.format_reader = Some(format_reader);
                             self.decoder = Some(decoder);
                             return Err(AudioError::UnsupportedFormat("Unsupported AudioBufferRef variant".to_string()));
                        }
                    };

                    // Restore ownership before returning success
                    self.format_reader = Some(format_reader);
                    self.decoder = Some(decoder);

                    // Return the owned buffer variant
                    trace!(target: LOG_TARGET, "decode_next_frame_owned: Returning DecodeRefResult::DecodedOwned(...)");
                    return Ok(owned_decoded_buffer);
                }
                Err(SymphoniaError::DecodeError(err)) => {
                    warn!(target: LOG_TARGET, "Symphonia decode error (skipping packet): {}", err);
                    // Continue decoding next packet
                }
                Err(e) => { // Other decoder errors
                    error!(target: LOG_TARGET, "Unexpected decoder error: {}", e);
                    // Restore ownership before returning error
                    self.format_reader = Some(format_reader);
                    self.decoder = Some(decoder);
                    trace!(target: LOG_TARGET, "decode_next_frame: Returning Err({:?}) (Unexpected decoder error)", e);
                    return Err(e.into());
                }
            }
        } // end loop
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

    // Removed the generic try_convert_buffer function as conversion is now handled
    // by returning an enum variant from decode_next_frame_owned.
}