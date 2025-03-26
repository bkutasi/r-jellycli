//! Integration tests for audio functionality
//!
//! These tests verify audio playback integration with other components.

use r_jellycli::audio::AlsaPlayer;
use std::error::Error;

#[cfg(test)]
mod audio_integration_tests {
    use super::*;

    /// Test basic player creation and initialization
    /// This test is marked as ignored since it requires actual hardware
    #[test]
    #[ignore]
    fn test_player_initialization() -> Result<(), Box<dyn Error>> {
        let mut player = AlsaPlayer::new("default");
        player.initialize()?;
        
        // If initialization succeeds, we've verified the player can connect to ALSA
        Ok(())
    }

    /// Test audio streaming from a test URL
    /// This test is marked as ignored as it requires network and hardware
    #[tokio::test]
    #[ignore]
    async fn test_audio_streaming() -> Result<(), Box<dyn Error>> {
        // This URL should point to a small audio sample for testing
        let test_url = "https://example.com/test-sample.mp3";
        
        let mut player = AlsaPlayer::new("default");
        player.initialize()?;
        
        // Only attempt to stream if initialization succeeded
        player.stream_from_url(test_url).await?;
        
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
