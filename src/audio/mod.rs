
pub mod alsa_handler;
pub mod alsa_writer;
pub mod decoder;
pub mod error;
pub mod loop_runner;
pub mod playback; // Keep the main playback orchestrator module
pub mod processor;
pub mod progress;
pub mod sample_converter;
pub mod state_manager;
pub mod stream_wrapper;

pub use error::AudioError;
pub use playback::{AudioPlaybackControl, PlaybackOrchestrator};
pub use progress::{PlaybackProgressInfo, SharedProgress};
pub use state_manager::OnFinishCallback;

#[cfg(test)]
mod tests;
