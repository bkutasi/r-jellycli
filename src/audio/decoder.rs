use crate::audio::error::AudioError;
use log::{debug, error, info, trace, warn};
use std::io;
use symphonia::core::audio::AudioBufferRef;
use symphonia::core::audio::{SignalSpec, AudioBuffer}; // Removed unused AudioBufferRef
use symphonia::core::codecs::{Decoder, DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::{FormatOptions, FormatReader, Packet}; // Removed unused Track
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use symphonia::core::units::TimeBase;
use std::any::TypeId; // Import TypeId for type checking
use tokio::sync::broadcast;
use tokio::task;

const LOG_TARGET: &str = "r_jellycli::audio::decoder";

/// Manages Symphonia format reading and decoding.
pub struct SymphoniaDecoder {
    format_reader: Option<Box<dyn FormatReader>>,
    decoder: Option<Box<dyn Decoder>>,
    track_id: u32,
    track_time_base: Option<TimeBase>,
    initial_spec: Option<SignalSpec>,
}

/// Represents the result of a decode operation.
// Make AudioBuffer generic over sample type S
pub enum DecodeResult<S: symphonia::core::sample::Sample> {
    /// Successfully decoded audio data.
    Decoded((AudioBuffer<S>, u64)), // Return owned AudioBuffer and timestamp
    /// End of stream reached.
    EndOfStream,
    /// A recoverable error occurred (e.g., decode error, skip packet).
    Skipped(String),
    /// Shutdown signal received.
    Shutdown,
}


impl SymphoniaDecoder {
    /// Creates and initializes a new Symphonia decoder from a media source stream.
    pub fn new(mss: MediaSourceStream) -> Result<Self, AudioError> {
        debug!(target: LOG_TARGET, "Setting up Symphonia format reader and decoder...");
        let hint = Hint::new();
        let meta_opts: MetadataOptions = Default::default();
        let fmt_opts: FormatOptions = Default::default();

        let probed = symphonia::default::get_probe().format(&hint, mss, &fmt_opts, &meta_opts)?;
        let format_reader = probed.format;

        let track = format_reader
            .tracks()
            .iter()
            .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
            .ok_or(AudioError::UnsupportedFormat("No suitable audio track found"))?
            .clone(); // Clone track info

        debug!(target: LOG_TARGET, "Found suitable audio track: ID={}, Codec={:?}", track.id, track.codec_params.codec);

        let decoder_opts = DecoderOptions::default();
        let decoder = symphonia::default::get_codecs().make(&track.codec_params, &decoder_opts)?;

        let initial_spec = SignalSpec::new(
            track.codec_params.sample_rate.ok_or(AudioError::MissingCodecParams("sample rate"))?,
            track.codec_params.channels.ok_or(AudioError::MissingCodecParams("channels map"))?,
        );

        debug!(target: LOG_TARGET, "Symphonia decoder created successfully. Initial Spec: {:?}", initial_spec);

        Ok(Self {
            format_reader: Some(format_reader),
            decoder: Some(decoder),
            track_id: track.id,
            track_time_base: track.codec_params.time_base,
            initial_spec: Some(initial_spec),
        })
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

        // Use spawn_blocking for the synchronous next_packet call
        let blocking_result = task::spawn_blocking(move || {
            // Move reader and decoder into the blocking task
            let packet_res = format_reader.next_packet();
            // Return ownership along with the result
            (format_reader, decoder, packet_res)
        }).await;

        match blocking_result {
            Ok((ret_reader, ret_decoder, Ok(packet))) => {
                trace!(target: LOG_TARGET, "Read packet: track={}, ts={}, duration={}", packet.track_id(), packet.ts(), packet.dur());
                // Return the reader/decoder received from the closure along with the packet
                Ok((ret_reader, ret_decoder, Some(packet)))
            },
            Ok((ret_reader, ret_decoder, Err(e))) => { // Return reader/decoder even on error
                // Check if the error indicates end of stream
                if matches!(e, symphonia::core::errors::Error::IoError(ref io_err) if io_err.kind() == std::io::ErrorKind::UnexpectedEof) {
                     debug!(target: LOG_TARGET, "End of stream reached.");
                     // Return the reader/decoder received from the closure, with None packet
                     Ok((ret_reader, ret_decoder, None))
                } else {
                     error!(target: LOG_TARGET, "Error reading next packet: {}", e);
                     // Propagate the error. Ownership was transferred, so reader/decoder state might be inconsistent.
                     // The caller must handle this potential inconsistency or loss of state.
                     // We cannot return the reader/decoder here as they are bound to the Ok variant.
                     Err(AudioError::SymphoniaError(e))
                }
            },
            Err(join_error) => {
                 // Task failed, reader/decoder ownership is lost
                 error!(target: LOG_TARGET, "Spawn blocking task for next_packet failed: {}", join_error);
                 Err(AudioError::TaskJoinError(join_error.to_string()))
            }
        } // End match blocking_result
    } // End fn read_next_packet

    /// Decodes the next available audio frame.
    /// Handles reading packets, decoding, and potential errors or stream end.
    /// Returns an owned AudioBuffer of the appropriate sample type.
    // Make decode_next_frame generic over sample type S
    pub async fn decode_next_frame<S: symphonia::core::sample::Sample + 'static>(
        &mut self,
        shutdown_rx: &mut broadcast::Receiver<()>,
    ) -> Result<DecodeResult<S>, AudioError> {
        // Take ownership of reader and decoder temporarily
        let mut format_reader = self.format_reader.take().ok_or(AudioError::InvalidState("Format reader not available".to_string()))?;
        let mut decoder = self.decoder.take().ok_or(AudioError::InvalidState("Decoder not available".to_string()))?;

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
                                return Ok(DecodeResult::EndOfStream); // Normal end
                            }
                            io::ErrorKind::Interrupted => {
                                info!(target: LOG_TARGET, "Playback interrupted (likely cancelled).");
                                return Ok(DecodeResult::Shutdown); // Treat cancellation as shutdown/stop
                            }
                            _ => {} // Fall through for other IO errors
                        }
                    }
                    // Handle other specific Symphonia errors
                    if let AudioError::SymphoniaError(SymphoniaError::ResetRequired) = e {
                        warn!(target: LOG_TARGET, "Symphonia decoder reset required (unhandled). Stopping.");
                        return Ok(DecodeResult::Skipped("Stream discontinuity (ResetRequired)".to_string()));
                    }

                    // Handle generic errors - ownership already lost
                    error!(target: LOG_TARGET, "Unhandled error reading next packet: {}", e);
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
                    match shutdown_rx.try_recv() {
                        Err(broadcast::error::TryRecvError::Empty) => Ok::<DecodeResult<S>, AudioError>(DecodeResult::EndOfStream),
                        _ => Ok::<DecodeResult<S>, AudioError>(DecodeResult::Shutdown),
                    }?; // Add explicit types
                    // This part needs refinement - how to reliably know if None was due to shutdown vs EOF?
                    // For now, assume EOF if shutdown wasn't explicitly caught earlier.
                    return Ok(DecodeResult::EndOfStream);
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
                            return Err(AudioError::UnsupportedFormat("Dynamic spec change"));
                        }
                    } else {
                         error!(target: LOG_TARGET, "Initial spec missing during decode check!");
                         self.format_reader = Some(format_reader);
                         self.decoder = Some(decoder);
                         return Err(AudioError::InvalidState("Initial spec missing".to_string()));
                    }

                    // --- Convert Buffer to Owned Generic Type ---
                    let decoded_buffer = match Self::try_convert_buffer::<S>(audio_buf_ref) {
                        Ok(buf) => buf,
                        Err(e) => {
                            error!(target: LOG_TARGET, "Failed to convert buffer: {}", e);
                            // Restore ownership before returning error
                            self.format_reader = Some(format_reader);
                            self.decoder = Some(decoder);
                            return Err(e);
                        }
                    };


                    // Restore ownership before returning success
                    self.format_reader = Some(format_reader);
                    self.decoder = Some(decoder);

                    // Return the owned buffer and its timestamp
                    return Ok(DecodeResult::Decoded((decoded_buffer, packet_ts)));
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

    /// Helper function to convert a Symphonia AudioBufferRef to an owned AudioBuffer<S>
    /// if the sample types match.
    fn try_convert_buffer<S: symphonia::core::sample::Sample + 'static>(
        audio_buf_ref: symphonia::core::audio::AudioBufferRef,
    ) -> Result<AudioBuffer<S>, AudioError> {
        // Use TypeId to check if the generic type S matches the concrete type in the buffer reference.
        match audio_buf_ref {
            AudioBufferRef::U8(_buf) => { // Prefixed unused variable
                if TypeId::of::<u8>() == TypeId::of::<S>() {
                    // Safety: We've checked the TypeId. A safer approach might involve
                    // creating a new buffer and copying, but `to_owned` should work if types match.
                    // We need to cast the result of to_owned() which is AudioBuffer<u8>.
                    // This requires unsafe or a more complex conversion. Let's stick to erroring.
                    // Ok(buf.to_owned() as AudioBuffer<S>) // This doesn't work directly.
                    Err(AudioError::UnsupportedFormat("Direct conversion for U8 not implemented safely yet"))

                } else {
                    Err(AudioError::UnsupportedFormat("Decoded U8, but expected different sample type"))
                }
            }
            AudioBufferRef::S16(buf) => {
                if TypeId::of::<i16>() == TypeId::of::<S>() {
                    // If S is i16, buf is &AudioBuffer<i16>, buf.to_owned() is AudioBuffer<i16>.
                    // We need to cast buf.to_owned() to AudioBuffer<S>.
                    // Since we checked TypeId, this *should* be safe, but Rust requires explicit handling.
                    // Let's assume `buf.to_owned()` gives the correct type if the check passes.
                    // The compiler might infer this, but let's be explicit with unsafe if needed,
                    // or find a library function. For now, assume `to_owned` works as expected.
                    // Revisit: Find the idiomatic safe way to do this cast based on TypeId.
                    // A simple `buf.to_owned()` should work if S is indeed i16.
                     // Use make_equivalent to create an owned buffer of type S
                     Ok(buf.make_equivalent::<S>())
                } else {
                    Err(AudioError::UnsupportedFormat("Decoded S16, but expected different sample type"))
                }
            }
             AudioBufferRef::S24(buf) => {
                 // S24 is often packed in i32. Check if S is i32.
                 if TypeId::of::<i32>() == TypeId::of::<S>() {
                     // Safety: We checked TypeId. This assumes S is i32.
                     // make_equivalent should handle the conversion if S=i32.
                     Ok(buf.make_equivalent::<S>())
                 } else {
                     Err(AudioError::UnsupportedFormat("Decoded S24(i32), but expected different sample type"))
                 }
             }
            AudioBufferRef::S32(buf) => {
                if TypeId::of::<i32>() == TypeId::of::<S>() {
                    Ok(buf.make_equivalent::<S>())
                } else {
                    Err(AudioError::UnsupportedFormat("Decoded S32, but expected different sample type"))
                }
            }
            AudioBufferRef::F32(buf) => {
                if TypeId::of::<f32>() == TypeId::of::<S>() {
                    Ok(buf.make_equivalent::<S>())
                } else {
                    Err(AudioError::UnsupportedFormat("Decoded F32, but expected different sample type"))
                }
            }
             AudioBufferRef::F64(buf) => {
                 if TypeId::of::<f64>() == TypeId::of::<S>() {
                     Ok(buf.make_equivalent::<S>())
                 } else {
                     Err(AudioError::UnsupportedFormat("Decoded F64, but expected different sample type"))
                 }
            }
            _ => Err(AudioError::UnsupportedFormat("Unsupported AudioBufferRef variant encountered")),
        }
    }
}