//! Integration tests for Jellyfin client functionality
//!
//! These tests verify that the Jellyfin client components work together correctly.

use r_jellycli::jellyfin::JellyfinClient;
use r_jellycli::config::Settings;
use std::error::Error;

#[cfg(test)]
mod jellyfin_integration_tests {
    use super::*;

    /// Test the client initialization with settings
    #[test]
    fn test_client_init_with_settings() {
        let settings = Settings {
            server_url: "https://test-server.com".to_string(),
            api_key: Some("test-api-key".to_string()),
            username: Some("test-user".to_string()),
            alsa_device: "default".to_string(),
            user_id: Some("test-user-id".to_string()),
        };
        
        let client = JellyfinClient::new(&settings.server_url)
            .with_api_key(&settings.api_key.unwrap())
            .with_user_id(&settings.user_id.unwrap());
            
        assert_eq!(client.get_server_url(), "https://test-server.com");
        assert_eq!(client.get_api_key().as_ref().unwrap(), "test-api-key");
        assert_eq!(client.get_user_id().as_ref().unwrap(), "test-user-id");
    }

    /// Test stream URL generation with different parameters
    #[test]
    fn test_stream_url_generation() {
        let client = JellyfinClient::new("https://test-server.com")
            .with_api_key("test-api-key");
            
        let url = client.get_stream_url("item123").unwrap();
        assert_eq!(url, "https://test-server.com/Audio/item123/stream?static=true&api_key=test-api-key");
    }
    
    /// This test requires a real Jellyfin server and is ignored by default
    #[tokio::test]
    #[ignore]
    async fn test_get_items() -> Result<(), Box<dyn Error>> {
        // This test would be run manually with `cargo test -- --ignored`
        // when a real Jellyfin server is available
        let client = JellyfinClient::new("https://your-server.com")
            .with_api_key("your-api-key")
            .with_user_id("your-user-id");
            
        let items = client.get_items().await?;
        assert!(!items.is_empty(), "Should retrieve at least one item from server");
        Ok(())
    }
}
