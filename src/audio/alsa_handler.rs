use crate::audio::error::AudioError;
use alsa::pcm::{Access, Format, HwParams, State as PcmState, PCM};
use alsa::{Direction, ValueOr};
use alsa::nix::errno::Errno;
use libc;
use tracing::{debug, error, info, warn}; // Replaced log with tracing
use tracing::instrument;
use std::ffi::CString;
use symphonia::core::audio::SignalSpec;

const LOG_TARGET: &str = "r_jellycli::audio::alsa_handler";

/// Manages the ALSA PCM device for audio output.
pub struct AlsaPcmHandler {
    device_name: String,
    pcm: Option<PCM>,
    requested_spec: Option<SignalSpec>, // Store the spec requested for initialization
    actual_rate: Option<u32>,      // Store the actual rate negotiated by ALSA
}

impl AlsaPcmHandler {
    /// Creates a new handler for the specified ALSA device.
    pub fn new(device_name: &str) -> Self {
        info!(target: LOG_TARGET, "Creating new AlsaPcmHandler for device: {}", device_name);
        AlsaPcmHandler {
            device_name: device_name.to_string(),
            pcm: None,
            requested_spec: None,
            actual_rate: None,
        }
    }

    /// Initializes the ALSA PCM device with the given specification.
    /// Closes any existing PCM device first.
    #[instrument(skip(self, spec), fields(device = %self.device_name, rate = spec.rate, channels = spec.channels.count()))]
    pub fn initialize(&mut self, spec: SignalSpec) -> Result<(), AudioError> {
        info!(
            target: LOG_TARGET,
            "Initializing ALSA PCM device '{}' with spec: rate={}, channels={}",
            self.device_name, spec.rate, spec.channels.count()
        );

        self.close(); // Ensure any existing PCM is closed first

        let device = CString::new(self.device_name.clone())
            .map_err(|e| AudioError::InitializationError(format!("Invalid device name: {}", e)))?;

        let pcm = PCM::open(&device, Direction::Playback, false)?; // Blocking mode

        // --- Hardware Parameters ---
        {
            let hwp = HwParams::any(&pcm)?;
            hwp.set_access(Access::RWInterleaved)?;
            hwp.set_format(Format::s16())?; // We convert everything to S16LE
            hwp.set_channels(spec.channels.count() as u32)?;

            match hwp.set_rate_near(spec.rate, ValueOr::Nearest) {
                Ok(_) => {
                    let actual_rate = hwp.get_rate()?;
                    if actual_rate != spec.rate {
                        warn!(
                            target: LOG_TARGET,
                            "ALSA rate negotiation: requested={}, actual={}",
                            spec.rate, actual_rate
                        );
                        // Store the actual rate
                        self.actual_rate = Some(actual_rate);
                    } else {
                        debug!(target: LOG_TARGET, "ALSA rate set successfully to {}", actual_rate);
                        // Store the actual rate even if it matches
                        self.actual_rate = Some(actual_rate);
                    }
                }
                Err(e) => {
                    error!(target: LOG_TARGET, "Failed to set ALSA rate near {}: {}", spec.rate, e);
                    return Err(AudioError::AlsaError(format!(
                        "Failed to set sample rate {}: {}",
                        spec.rate, e
                    )));
                }
            }
            pcm.hw_params(&hwp)?;
            debug!(target: LOG_TARGET, "ALSA hardware parameters applied.");

            // --- Software Parameters ---
            let swp = pcm.sw_params_current()?;
            let buffer_size = hwp.get_buffer_size()?;
            let period_size = hwp.get_period_size()?;
            swp.set_start_threshold(buffer_size - period_size)?;
            // swp.set_avail_min(period_size)?; // Consider for lower latency
            pcm.sw_params(&swp)?;
            debug!(target: LOG_TARGET, "ALSA software parameters applied (buffer={}, period={}).", buffer_size, period_size);
        }

        self.pcm = Some(pcm);
        self.requested_spec = Some(spec); // Store the requested spec
        info!(target: LOG_TARGET, "ALSA initialized successfully.");
        Ok(())
    }

    /// Writes a buffer of S16LE interleaved samples, handling ALSA underruns.
    /// Returns Ok(frames_written) or Err on unrecoverable error.
    /// Note: Returns Ok(0) if an underrun occurred and was recovered.
    #[instrument(skip(self, buffer), fields(frames = buffer.len() / self.requested_spec.map_or(2, |s| s.channels.count())))] // Calculate frames based on stored spec
    pub fn write_s16_buffer(&self, buffer: &[i16]) -> Result<usize, AudioError> {
        let pcm = self.pcm.as_ref().ok_or(AudioError::InvalidState("PCM not initialized for writing".to_string()))?;
        let io = pcm.io_i16()?;

        match io.writei(buffer) {
            Ok(frames_written) => Ok(frames_written),
            Err(e) if e.errno() == Errno::EPIPE => { // Underrun
                warn!(target: LOG_TARGET, "ALSA buffer underrun (EPIPE), attempting recovery...");
                match pcm.recover(libc::EPIPE, true) { // Blocking recovery
                    Ok(()) => {
                        debug!(target: LOG_TARGET, "ALSA recovery successful.");
                        Ok(0) // Indicate recovery happened, wrote 0 frames *in this attempt*
                    }
                    Err(recover_err) => {
                        error!(target: LOG_TARGET, "ALSA recovery failed: {}", recover_err);
                        Err(AudioError::AlsaError(format!("ALSA recovery failed: {}", recover_err)))
                    }
                }
            }
            Err(e) => { // Other ALSA errors
                error!(target: LOG_TARGET, "ALSA write error: {}", e);
                Err(AudioError::AlsaError(e.to_string()))
            }
        }
    }

    /// Attempts to drain the ALSA buffer. Call this after the stream ends.
    pub fn drain(&self) -> Result<(), AudioError> {
        if let Some(pcm) = &self.pcm {
             if pcm.state() == PcmState::Running || pcm.state() == PcmState::Prepared {
                debug!(target: LOG_TARGET, "Draining ALSA buffer.");
                match pcm.drain() {
                    Ok(_) => {
                        debug!(target: LOG_TARGET, "ALSA drain successful.");
                        Ok(())
                    },
                    Err(e) => {
                        warn!(target: LOG_TARGET, "Error draining ALSA buffer: {}", e);
                        Err(e.into()) // Convert alsa::Error to AudioError
                    }
                }
            } else {
                 debug!(target: LOG_TARGET, "ALSA not running or prepared, skipping drain.");
                 Ok(())
            }
        } else {
            debug!(target: LOG_TARGET, "PCM not initialized, skipping drain.");
            Ok(())
        }
    }


    /// Closes the ALSA PCM device if it's open, attempting to drain first.
    pub fn close(&mut self) {
        if let Some(pcm) = self.pcm.take() { // Take ownership to drop
            debug!(target: LOG_TARGET, "Closing ALSA PCM device (state: {:?})...", pcm.state());
            // Attempt drain, but ignore errors during close as we are shutting down anyway.
            if pcm.state() == PcmState::Running || pcm.state() == PcmState::Prepared {
                debug!(target: LOG_TARGET, "Dropping ALSA PCM stream immediately during close.");
                match pcm.drop() { // Use drop() for immediate stop instead of drain()
                    Ok(_) => debug!(target: LOG_TARGET, "ALSA drop successful during close."),
                    Err(e) => warn!(target: LOG_TARGET, "Error dropping ALSA buffer during close (ignored): {}", e),
                }
            }
            // PCM is dropped here, closing the device
            debug!(target: LOG_TARGET, "ALSA PCM closed.");
        }
        self.requested_spec = None; // Clear stored spec
        self.actual_rate = None; // Clear actual rate
    }

    /// Returns the current state of the PCM device.
    pub fn state(&self) -> PcmState {
        self.pcm.as_ref().map_or(PcmState::Open, |p| p.state())
    }

    /// Returns the actual sample rate negotiated with ALSA during initialization.
    pub fn get_actual_rate(&self) -> Option<u32> {
        self.actual_rate
    }
}

impl Drop for AlsaPcmHandler {
    fn drop(&mut self) {
        // Ensure resources are released when the handler goes out of scope
        debug!(target: LOG_TARGET, "Dropping AlsaPcmHandler.");
        self.close();
    }
}