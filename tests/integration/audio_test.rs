//! Integration tests for audio functionality
//!
//! These tests verify audio playback integration with other components.

use r_jellycli::audio::PlaybackOrchestrator;
use std::error::Error;
use tokio::sync::broadcast; // Add import for broadcast channel
#[cfg(test)]
mod audio_integration_tests {
    use super::*;

    // Removed obsolete test_player_initialization as initialize() is no longer public

    /// Test audio streaming from a test URL
    /// This test is marked as ignored as it requires network and hardware
    #[tokio::test]
    #[ignore]
    async fn test_audio_streaming() -> Result<(), Box<dyn Error>> {
        // This URL should point to a small audio sample for testing
        let test_url = "https://www.soundhelix.com/examples/mp3/SoundHelix-Song-1.mp3"; // Use a real MP3 URL for testing if possible
        
        let mut player = PlaybackOrchestrator::new("default");
        // Initialization is now part of stream_decode_and_play

        // Create a dummy shutdown channel for the test
        let (_shutdown_tx, shutdown_rx) = broadcast::channel(1);
        
        // Call the correct method with required arguments
        player.stream_decode_and_play(test_url, None, shutdown_rx).await?;
        
        // If we get here without errors, the test passes
        Ok(())
    }

    /// Test the mock version of the audio player for unit testing
    #[test]
    fn test_audio_error_handling() {
        use r_jellycli::audio::AudioError;
        
        // Test that error types work correctly
        let error = AudioError::AlsaError("Test error".to_string());
        assert_eq!(format!("{}", error), "ALSA error: Test error");
        
        // Additional error handling tests could go here
    }
}
