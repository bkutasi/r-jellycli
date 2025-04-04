// src/audio/mod.rs

// Declare the new modules
pub mod alsa_handler;
pub mod decoder;
pub mod error;
pub mod format_converter;
pub mod playback; // Keep the main playback orchestrator module
pub mod progress;
pub mod stream_wrapper;
pub mod sample_converter;

// Re-export key types for easier access from outside `audio` module
pub use error::AudioError;
pub use playback::PlaybackOrchestrator; // Renamed from AlsaPlayer
pub use progress::{PlaybackProgressInfo, SharedProgress, PROGRESS_UPDATE_INTERVAL};

// Keep tests module if it exists
#[cfg(test)]
mod tests;
