use crate::audio::{
    error::AudioError,
    sample_converter,
};
use rubato::Resampler;
use rubato::SincFixedIn;
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
    #[instrument(skip(self, audio_buffer), fields(buffer_type = std::any::type_name::<S>(), frames = audio_buffer.frames(), is_last))]
    pub async fn process_buffer<S: Sample + std::fmt::Debug + Send + Sync + 'static>(
        &self,
        audio_buffer: AudioBuffer<S>,
        is_last: bool, // Added flag
    ) -> Result<Option<Vec<i16>>, AudioError> {
        trace!(target: LOG_TARGET, "Processing buffer: {} frames", audio_buffer.frames());

        let s16_vec: Vec<i16>;

        // --- Resampling Logic ---
        if let Some(resampler_arc) = self.resampler.as_ref() {
            let mut resampler = resampler_arc.lock().await;
            trace!(target: LOG_TARGET, "Resampling buffer (is_last: {})...", is_last);

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

            // 2. Process based on whether this is the last buffer
            if is_last {
                // Process the entire remaining buffer at once and flush
                trace!(target: LOG_TARGET, "Processing last buffer ({} frames) with process_last...", total_input_frames);
                // Convert Vec<Vec<f32>> to Vec<&[f32]> for process_partial
                let input_slices: Vec<&[f32]> = f32_input_vecs.iter().map(|v| v.as_slice()).collect();

                let resampler_mut = &mut *resampler; // Explicit mutable dereference
                // Use process_partial for the last chunk
                match resampler_mut.process_partial(Some(&input_slices), None) {
                    Ok(output_chunk) => {
                        if !output_chunk.is_empty() && !output_chunk[0].is_empty() {
                            trace!(target: LOG_TARGET, "Resampler process_last output size: {} frames", output_chunk[0].len());
                            // Directly assign, as this is the final output
                            accumulated_output_vecs = output_chunk;
                        } else {
                             trace!(target: LOG_TARGET, "Resampler process_last output is empty.");
                             // Ensure accumulated_output_vecs is empty if output is empty
                             accumulated_output_vecs.iter_mut().for_each(|v| v.clear());
                        }
                    }
                    Err(e) => {
                        error!(target: LOG_TARGET, "Resampling failed during process_last: {}", e);
                        // Treat as skippable error for now, might need better handling
                        return Ok(None);
                    }
                }
            } else {
                // Process the f32 input vectors in chunks (existing logic)
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
                            input_chunk.push(&[]); // Push empty slice on error to match expected structure
                        }
                    }

                    trace!(target: LOG_TARGET, "Processing chunk: frames {}..{} (size {})", processed_frames, end_frame - 1, current_chunk_size);

                    match resampler.process(&input_chunk, None) {
                        Ok(output_chunk) => {
                            if !output_chunk.is_empty() && !output_chunk[0].is_empty() {
                                trace!(target: LOG_TARGET, "Resampler output chunk size: {} frames", output_chunk[0].len());
                                for ch in 0..num_channels {
                                    if ch < output_chunk.len() && ch < accumulated_output_vecs.len() { // Bounds check
                                        accumulated_output_vecs[ch].extend_from_slice(&output_chunk[ch]);
                                    }
                                }
                            } else {
                                trace!(target: LOG_TARGET, "Resampler output chunk is empty.");
                            }
                        }
                        Err(e) => {
                            error!(target: LOG_TARGET, "Resampling failed during chunk processing: {}", e);
                            return Ok(None); // Treat as skippable error
                        }
                    }
                    processed_frames = end_frame;
                }
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

    /// Returns the number of output channels the processor is configured for.
    pub fn output_channels(&self) -> usize {
        self.num_channels
    }

    // Removed flush_resampler function as process_last handles flushing.
}
