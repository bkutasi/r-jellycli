use crate::audio::error::AudioError;
use symphonia::core::audio::Signal;
use symphonia::core::audio::AudioBuffer;
use symphonia::core::sample::Sample;
use std::any::TypeId;
use tracing::{trace, warn};

const LOG_TARGET: &str = "r_jellycli::audio::sample_converter";

/// Converts a generic Symphonia AudioBuffer into an interleaved S16LE Vec.
pub fn convert_buffer_to_s16<S: Sample + 'static>(
    audio_buffer: AudioBuffer<S>,
) -> Result<Vec<i16>, AudioError> {
    let spec = audio_buffer.spec();
    let num_frames = audio_buffer.frames();
    let num_channels = spec.channels.count();
    let mut s16_vec = vec![0i16; num_frames * num_channels];

    let type_id_s = TypeId::of::<S>();
    let planes_data = audio_buffer.planes();
    let channel_planes = planes_data.planes();

    trace!(target: LOG_TARGET, "Converting buffer ({} frames, {} channels, type: {:?}) to S16LE", num_frames, num_channels, type_id_s);

    // --- Conversion Logic (adapted from playback.rs) ---
    if type_id_s == TypeId::of::<i16>() {
        trace!(target: LOG_TARGET, "Input is S16");
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
        // Assumes S32 or S24 packed in i32. Convert to S16 by right-shifting.
        if num_channels == 1 {
            let plane_i32 = unsafe { std::slice::from_raw_parts(channel_planes[0].as_ptr() as *const i32, num_frames) };
            for frame in 0..num_frames {
                s16_vec[frame] = (plane_i32[frame] >> 16) as i16;
            }
        } else {
            for frame in 0..num_frames {
                for ch in 0..num_channels {
                    let sample_i32 = unsafe { *(channel_planes[ch].as_ptr() as *const i32).add(frame) };
                    s16_vec[frame * num_channels + ch] = (sample_i32 >> 16) as i16;
                }
            }
        }
    } else if type_id_s == TypeId::of::<f32>() {
        trace!(target: LOG_TARGET, "Input is F32");
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
         return Err(AudioError::UnsupportedFormat("Cannot convert decoded format to S16".to_string()));
    }

    Ok(s16_vec)
}

/// Converts a generic Symphonia AudioBuffer into Vec<Vec<f32>> suitable for Rubato.
pub fn convert_buffer_to_f32_vecs<S: Sample + 'static>(
    audio_buffer: AudioBuffer<S>,
) -> Result<Vec<Vec<f32>>, AudioError> {
    let spec = audio_buffer.spec();
    let num_frames = audio_buffer.frames();
    let num_channels = spec.channels.count();
    let mut f32_vecs: Vec<Vec<f32>> = vec![vec![0.0f32; num_frames]; num_channels];

    let type_id_s = TypeId::of::<S>();
    let planes_data = audio_buffer.planes();
    let channel_planes = planes_data.planes();

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
                f32_vecs[ch][frame] = (plane_i32[frame] as f64 / 2147483648.0) as f32;
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
                f32_vecs[ch][frame] = plane_f64[frame] as f32;
            }
        }
    } else {
        warn!(target: LOG_TARGET, "Unsupported sample type {:?} for F32 conversion.", TypeId::of::<S>());
        return Err(AudioError::UnsupportedFormat("Cannot convert decoded format to F32 for resampling".to_string()));
    }

    Ok(f32_vecs)
}

/// Converts Vec<Vec<f32>> (output from Rubato) into an interleaved S16LE Vec.
pub fn convert_f32_vecs_to_s16(
    f32_vecs: Vec<Vec<f32>>,
) -> Result<Vec<i16>, AudioError> {
    if f32_vecs.is_empty() || f32_vecs[0].is_empty() {
        return Ok(Vec::new());
    }

    let num_channels = f32_vecs.len();
    let num_frames = f32_vecs[0].len();
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
                s16_vec[frame * num_channels + ch] = 0;
            }
        }
    }

    Ok(s16_vec)
}