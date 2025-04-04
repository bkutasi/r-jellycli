//! Integration tests for audio functionality
//!
//! These tests verify audio playback integration with other components.

use r_jellycli::audio::PlaybackOrchestrator;
use std::error::Error;
use tokio::sync::broadcast;
#[cfg(test)]
mod audio_integration_tests {
    use super::*;


    /// Test audio streaming from a test URL
    /// This test is marked as ignored as it requires network and hardware
    #[tokio::test]
    #[ignore]
    async fn test_audio_streaming() -> Result<(), Box<dyn Error>> {
        let test_url = "https://www.soundhelix.com/examples/mp3/SoundHelix-Song-1.mp3";
        
        let mut player = PlaybackOrchestrator::new("default");

        let (_shutdown_tx, shutdown_rx) = broadcast::channel(1);
        
        player.stream_decode_and_play(test_url, None, shutdown_rx).await?;
        
        Ok(())
    }

    #[test]
    fn test_audio_error_handling() {
        use r_jellycli::audio::AudioError;
        
        let error = AudioError::AlsaError("Test error".to_string());
        assert_eq!(format!("{}", error), "ALSA error: Test error");
        
    }
}
