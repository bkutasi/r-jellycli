use std::error::Error;
use std::io;
use symphonia::core::errors::Error as SymphoniaError;

/// Error types specific to audio playback.
#[derive(Debug)]
pub enum AudioError {
    AlsaError(String),
    StreamError(String),
    DecodingError(String),
    SymphoniaError(SymphoniaError),
    IoError(io::Error),
    NetworkError(reqwest::Error),
    InvalidState(String), // Changed to accept owned String
    UnsupportedFormat(String), // Changed to accept owned String
    MissingCodecParams(&'static str),
    TaskJoinError(String),
    InitializationError(String),
    PlaybackError(String), // Added for general playback issues
    ResamplingError(String), // Added for resampling errors
    UnsupportedOperation(String), // Added for operations not supported by the backend
}

impl std::fmt::Display for AudioError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AudioError::AlsaError(e) => write!(f, "ALSA error: {}", e),
            AudioError::StreamError(e) => write!(f, "Streaming error: {}", e),
            AudioError::DecodingError(e) => write!(f, "Decoding error: {}", e),
            AudioError::SymphoniaError(e) => write!(f, "Symphonia error: {}", e),
            AudioError::IoError(e) => write!(f, "I/O error: {}", e),
            AudioError::NetworkError(e) => write!(f, "Network error: {}", e),
            AudioError::InvalidState(s) => write!(f, "Invalid state: {}", s), // Display impl remains the same
            AudioError::UnsupportedFormat(s) => write!(f, "Unsupported format: {}", s),
            AudioError::MissingCodecParams(s) => write!(f, "Missing codec parameters: {}", s),
            AudioError::TaskJoinError(e) => write!(f, "Async task join error: {}", e),
            AudioError::InitializationError(e) => write!(f, "Initialization error: {}", e),
            AudioError::PlaybackError(e) => write!(f, "Playback error: {}", e), // Added display for PlaybackError
            AudioError::ResamplingError(e) => write!(f, "Resampling error: {}", e), // Added display for ResamplingError
            AudioError::UnsupportedOperation(e) => write!(f, "Unsupported operation: {}", e), // Added display for UnsupportedOperation
        }
    }
}

impl Error for AudioError {}

// --- From Implementations for AudioError ---

impl From<alsa::Error> for AudioError {
    fn from(e: alsa::Error) -> Self {
        AudioError::AlsaError(e.to_string())
    }
}

impl From<SymphoniaError> for AudioError {
    fn from(e: SymphoniaError) -> Self {
        AudioError::SymphoniaError(e)
    }
}

impl From<io::Error> for AudioError {
    fn from(e: io::Error) -> Self {
        AudioError::IoError(e)
    }
}

impl From<reqwest::Error> for AudioError {
    fn from(e: reqwest::Error) -> Self {
        AudioError::NetworkError(e)
    }
}

impl From<tokio::task::JoinError> for AudioError {
    fn from(e: tokio::task::JoinError) -> Self {
        AudioError::TaskJoinError(e.to_string())
    }
}