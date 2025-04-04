use crate::audio::{
    error::AudioError,
    sample_converter,
};
use rubato::{Resampler, SincFixedIn};
use std::sync::Arc;
use symphonia::core::{
    audio::{AudioBuffer, Signal},
    sample::Sample,
};
use tokio::sync::Mutex as TokioMutex;
use tracing::{error, trace, warn, instrument};

const LOG_TARGET: &str = "r_jellycli::audio::processor";

/// Handles audio processing tasks like resampling and sample format conversion.
pub struct AudioProcessor {
    resampler: Option<Arc<TokioMutex<SincFixedIn<f32>>>>,
    num_channels: usize,
}

impl AudioProcessor {
    /// Creates a new AudioProcessor.
    /// Takes an optional resampler if resampling is needed.
    pub fn new(resampler: Option<Arc<TokioMutex<SincFixedIn<f32>>>>, num_channels: usize) -> Self {
        Self { resampler, num_channels }
    }

    /// Processes an audio buffer: resamples (if configured) and converts to S16.
    /// Returns `Ok(None)` if the buffer should be skipped (e.g., conversion error, empty buffer).
    #[instrument(skip(self, audio_buffer), fields(buffer_type = std::any::type_name::<S>(), frames = audio_buffer.frames()))]
    pub async fn process_buffer<S: Sample + std::fmt::Debug + Send + Sync + 'static>(
        &self,
        audio_buffer: AudioBuffer<S>,
    ) -> Result<Option<Vec<i16>>, AudioError> {
        trace!(target: LOG_TARGET, "Processing buffer: {} frames", audio_buffer.frames());

        let s16_vec: Vec<i16>;

        // --- Resampling Logic ---
        if let Some(resampler_arc) = self.resampler.as_ref() {
            let mut resampler = resampler_arc.lock().await;
            trace!(target: LOG_TARGET, "Resampling buffer...");

            // 1. Convert the entire input buffer to f32 vectors first
            let f32_input_vecs = match sample_converter::convert_buffer_to_f32_vecs(audio_buffer) {
                Ok(vecs) => vecs,
                Err(e) => {
                    warn!(target: LOG_TARGET, "Failed to convert buffer to F32 for resampling: {}. Skipping buffer.", e);
                    return Ok(None);
                }
            };

            if f32_input_vecs.is_empty() || f32_input_vecs[0].is_empty() {
                trace!(target: LOG_TARGET, "Input buffer is empty after F32 conversion, skipping resampling.");
                return Ok(None);
            }

            let num_channels = f32_input_vecs.len();
            let total_input_frames = f32_input_vecs[0].len();
            let mut processed_frames = 0;
            let mut accumulated_output_vecs: Vec<Vec<f32>> = vec![Vec::new(); num_channels];

            // 2. Process the f32 input vectors in chunks
            // Use input_frames_next() to determine how many frames the resampler needs
            while processed_frames < total_input_frames {
                 let needed_input_frames = resampler.input_frames_next();
                 let remaining_frames = total_input_frames - processed_frames;
                 let current_chunk_size = remaining_frames.min(needed_input_frames);

                if current_chunk_size == 0 {
                    trace!(target: LOG_TARGET, "No more input frames to process in this loop iteration.");
                    break;
                }

                let end_frame = processed_frames + current_chunk_size;

                let mut input_chunk: Vec<&[f32]> = Vec::with_capacity(num_channels);
                for ch in 0..num_channels {
                     if processed_frames < f32_input_vecs[ch].len() && end_frame <= f32_input_vecs[ch].len() {
                        input_chunk.push(&f32_input_vecs[ch][processed_frames..end_frame]);
                    } else {
                        error!(target: LOG_TARGET, "Inconsistent input vector length during chunking at channel {}, frame {}. Input len: {}, end_frame: {}", ch, processed_frames, f32_input_vecs[ch].len(), end_frame);
                        input_chunk.push(&[]);
                    }
                }

                trace!(target: LOG_TARGET, "Processing chunk: frames {}..{} (size {})", processed_frames, end_frame - 1, current_chunk_size);

                match resampler.process(&input_chunk, None) {
                    Ok(output_chunk) => {
                        if !output_chunk.is_empty() && !output_chunk[0].is_empty() {
                            trace!(target: LOG_TARGET, "Resampler output chunk size: {} frames", output_chunk[0].len());
                            for ch in 0..num_channels {
                                if ch < output_chunk.len() {
                                    accumulated_output_vecs[ch].extend_from_slice(&output_chunk[ch]);
                                }
                            }
                        } else {
                             trace!(target: LOG_TARGET, "Resampler output chunk is empty.");
                        }
                    }
                    Err(e) => {
                        error!(target: LOG_TARGET, "Resampling failed during chunk processing: {}", e);
                        return Ok(None);
                    }
                }
                processed_frames = end_frame;
            }

            // 3. Convert the accumulated resampled f32 vectors to s16
            trace!(target: LOG_TARGET, "Converting accumulated resampled output ({} frames) to S16...", accumulated_output_vecs.get(0).map_or(0, |v| v.len()));
            s16_vec = match sample_converter::convert_f32_vecs_to_s16(accumulated_output_vecs) {
                Ok(vec) => vec,
                Err(e) => {
                    warn!(target: LOG_TARGET, "Failed to convert accumulated resampled F32 buffer to S16: {}. Skipping buffer.", e);
                    return Ok(None);
                }
            };

        } else { // No resampler needed
            trace!(target: LOG_TARGET, "No resampling needed, converting directly to S16...");
            // Convert directly from the input buffer
            s16_vec = match sample_converter::convert_buffer_to_s16(audio_buffer) {
                Ok(vec) => vec,
                Err(e) => {
                    warn!(target: LOG_TARGET, "Failed to convert buffer to S16: {}. Skipping buffer.", e);
                    return Ok(None);
                }
            };
        }

        // --- Check if buffer is empty ---
        if s16_vec.is_empty() {
            trace!(target: LOG_TARGET, "Skipping empty buffer after conversion/resampling.");
            return Ok(None);
        }

        Ok(Some(s16_vec))
    }

    /// Flushes the resampler (if it exists) and returns the final S16 samples.
    /// Returns `Ok(None)` if no resampler exists or flushing yields no data/error.
    #[instrument(skip(self))]
    pub async fn flush_resampler(&self) -> Result<Option<Vec<i16>>, AudioError> {
        if let Some(resampler_arc) = self.resampler.as_ref() {
            let mut resampler = resampler_arc.lock().await;
            trace!(target: LOG_TARGET, "Flushing resampler...");

            // Flush by processing empty input slices
            let empty_inputs: Vec<&[f32]> = vec![&[]; self.num_channels]; // Use stored channel count
            match resampler.process(&empty_inputs, None) {
                Ok(f32_output_vecs) => {
                    if !f32_output_vecs.is_empty() && !f32_output_vecs[0].is_empty() {
                        trace!(target: LOG_TARGET, "Resampler flush successful, got {} output frames.", f32_output_vecs.get(0).map_or(0, |v| v.len()));
                        match sample_converter::convert_f32_vecs_to_s16(f32_output_vecs) {
                            Ok(s16_vec) => {
                                if s16_vec.is_empty() {
                                    trace!(target: LOG_TARGET, "Flushed resampler buffer converted to empty S16 buffer.");
                                    Ok(None)
                                } else {
                                    trace!(target: LOG_TARGET, "Returning flushed S16 buffer ({} samples).", s16_vec.len());
                                    Ok(Some(s16_vec))
                                }
                            }
                            Err(e) => {
                                error!(target: LOG_TARGET, "Failed to convert flushed resampler buffer to S16: {}", e);
                                Err(e)
                            }
                        }
                    } else {
                        trace!(target: LOG_TARGET, "Resampler flush returned no frames.");
                        Ok(None)
                    }
                }
                Err(e) => {
                    error!(target: LOG_TARGET, "Resampler flush (process with empty input) failed: {}", e);
                    Err(AudioError::ResamplingError(format!("Resampler flush failed: {}", e)))
                }
            }
        } else {
            trace!(target: LOG_TARGET, "No resampler active, no flush needed.");
            Ok(None)
        }
    }
}