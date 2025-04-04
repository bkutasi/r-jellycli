use tracing::warn;
// Use AudioBufferRef for dynamic dispatch based on the underlying buffer type
use symphonia::core::audio::AudioBufferRef; // Removed unused SignalSpec


const LOG_TARGET: &str = "r_jellycli::audio::format_converter";

/// Converts various Symphonia audio buffer formats (via ref) to interleaved S16LE samples.
/// Accepts an AudioBufferRef, which can be obtained from an owned AudioBuffer using `.as_ref()`.
/// Returns None if the format is unsupported.
pub fn convert_ref_to_s16(audio_buf_ref: AudioBufferRef) -> Option<Vec<i16>> {
    let spec = audio_buf_ref.spec();
    let num_frames = audio_buf_ref.frames();
    let num_channels = spec.channels.count();
    if num_channels == 0 || num_frames == 0 {
        return Some(Vec::new());
    }

    let mut s16_vec = vec![0i16; num_frames * num_channels];

    // Helper macro for interleaving different sample types
    macro_rules! interleave {
        ($planes:expr, $frame:ident, $ch:ident, $sample_type:ty, $conversion_expr:expr) => {
            let channel_planes = $planes.planes();
            // Bounds check: Ensure plane count matches channel count
            if channel_planes.len() != num_channels {
                 warn!(target: LOG_TARGET, "Plane count ({}) does not match channel count ({})", channel_planes.len(), num_channels);
                 return None;
            }
            // Bounds check: Ensure frame count matches in first plane (if channels > 0)
            if num_channels > 0 && channel_planes[0].len() != num_frames {
                 warn!(target: LOG_TARGET, "Frame count in plane 0 ({}) does not match buffer frame count ({})", channel_planes[0].len(), num_frames);
                 return None;
            }

            if num_channels == 1 {
                let channel_data = channel_planes[0];
                for $frame in 0..num_frames {
                    let sample: $sample_type = channel_data[$frame];
                    s16_vec[$frame] = $conversion_expr(sample);
                }
            } else {
                for $frame in 0..num_frames {
                    for $ch in 0..num_channels {
                         // Bounds check: Ensure frame count matches in each plane
                         if channel_planes[$ch].len() != num_frames {
                              warn!(target: LOG_TARGET, "Frame count in plane {} ({}) does not match buffer frame count ({})", $ch, channel_planes[$ch].len(), num_frames);
                              return None;
                         }
                        let sample: $sample_type = channel_planes[$ch as usize][$frame];
                        let idx = $frame * num_channels + $ch;
                        s16_vec[idx] = $conversion_expr(sample);
                    }
                }
            }
        };
    }

    match audio_buf_ref {
        AudioBufferRef::U8(buf) => { let planes = buf.planes(); interleave!(planes, frame, ch, u8, |s: u8| ((s as i16 - 128) * 256)); },
        AudioBufferRef::S16(buf) => { let planes = buf.planes(); interleave!(planes, frame, ch, i16, |s: i16| s); },
        AudioBufferRef::S24(buf) => { // S24 is special, packed in i32
            let planes = buf.planes();
            let channel_planes = planes.planes();
            if channel_planes.len() != num_channels { warn!(target: LOG_TARGET, "S24 Plane count mismatch"); return None; }
            if num_channels > 0 && channel_planes[0].len() != num_frames { warn!(target: LOG_TARGET, "S24 Frame count mismatch (Plane 0)"); return None; }

            if num_channels == 1 {
                let channel_data = channel_planes[0];
                for i in 0..num_frames {
                    // Access .0 for the i32 value within the S24 Sample struct
                    s16_vec[i] = (channel_data[i].0 >> 8) as i16;
                }
            } else {
                for frame in 0..num_frames {
                    for ch in 0..num_channels {
                         if channel_planes[ch].len() != num_frames { warn!(target: LOG_TARGET, "S24 Frame count mismatch (Plane {})", ch); return None; }
                        let sample = channel_planes[ch as usize][frame];
                        let idx = frame * num_channels + ch;
                         // Access .0 for the i32 value within the S24 Sample struct
                        s16_vec[idx] = (sample.0 >> 8) as i16;
                    }
                }
            }
        }
        AudioBufferRef::S32(buf) => { let planes = buf.planes(); interleave!(planes, frame, ch, i32, |s: i32| (s >> 16) as i16); },
        AudioBufferRef::F32(buf) => { let planes = buf.planes(); interleave!(planes, frame, ch, f32, |s: f32| (s * 32767.0).clamp(-32768.0, 32767.0) as i16); },
        // Add F64 if needed and supported by ALSA target format (S16LE here)
        _ => {
            warn!(target: LOG_TARGET, "Unsupported audio format for S16LE conversion: {:?}", spec);
            return None;
        }
    }
    Some(s16_vec)
}