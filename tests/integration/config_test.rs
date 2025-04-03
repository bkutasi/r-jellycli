//! Integration tests for configuration management
//!
//! These tests verify that the configuration system works correctly
//! across module boundaries.

use r_jellycli::config::Settings;
use std::error::Error;
use tempfile::tempdir;

#[cfg(test)]
mod config_integration_tests {
    use super::*;

    /// Test complete configuration workflow
    #[test]
    fn test_config_lifecycle() -> Result<(), Box<dyn Error>> {
        // Create a temporary directory for test
        let dir = tempdir()?;
        let config_path = dir.path().join("config.json");
        
        // Create settings with test values
        let mut settings = Settings::default();
        settings.server_url = "https://jellyfin-server.example.com".to_string();
        settings.api_key = Some("integration-test-api-key".to_string());
        settings.username = Some("integration-test-user".to_string());
        settings.user_id = Some("integration-test-user-id".to_string());
        settings.alsa_device = "test-audio-device".to_string();
        
        // Validate and save settings
        settings.validate()?;
        settings.save(&config_path)?;
        
        // Load settings back
        let loaded_settings = Settings::load(&config_path)?;
        
        // Verify loaded settings match what we saved
        assert_eq!(loaded_settings.server_url, "https://jellyfin-server.example.com");
        assert_eq!(loaded_settings.api_key, Some("integration-test-api-key".to_string()));
        assert_eq!(loaded_settings.username, Some("integration-test-user".to_string()));
        assert_eq!(loaded_settings.user_id, Some("integration-test-user-id".to_string()));
        assert_eq!(loaded_settings.alsa_device, "test-audio-device");
        
        // Test overriding settings
        let mut updated_settings = loaded_settings;
        updated_settings.server_url = "https://updated-server.example.com".to_string();
        updated_settings.save(&config_path)?;
        
        // Load again and verify updates
        let reloaded_settings = Settings::load(&config_path)?;
        assert_eq!(reloaded_settings.server_url, "https://updated-server.example.com");
        
        Ok(())
    }
    
    /// Test invalid configuration handling
    #[test]
    fn test_invalid_config_validation() {
        // Test with empty server URL
        let invalid_settings = Settings {
            server_url: "".to_string(),
            api_key: Some("test-key".to_string()),
            username: None,
            alsa_device: "default".to_string(),
            user_id: None,
            device_id: None,
        };
        
        let result = invalid_settings.validate();
        assert!(result.is_err());
        
        if let Err(e) = result {
            assert!(e.to_string().contains("URL cannot be empty"));
        }
        
        // Test with missing authentication
        let no_auth_settings = Settings {
            server_url: "https://example.com".to_string(),
            api_key: None,
            username: None,
            alsa_device: "default".to_string(),
            user_id: None,
            device_id: None,
        };
        
        // Validation requires either API key or username
        assert!(no_auth_settings.validate().is_err());
    }
}
