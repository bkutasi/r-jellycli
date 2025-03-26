//! Tests for configuration management module

#[cfg(test)]
mod tests {
    use super::super::*;
    
    
    use tempfile::tempdir;
    
    #[test]
    fn test_default_settings() {
        let settings = Settings::default();
        assert_eq!(settings.server_url, "http://localhost:8096");
        assert!(settings.api_key.is_none());
        assert!(settings.username.is_none());
        assert_eq!(settings.alsa_device, "default");
        assert!(settings.user_id.is_none());
    }
    
    #[test]
    fn test_settings_save_and_load() -> Result<(), Box<dyn std::error::Error>> {
        // Create a temporary directory for our test
        let dir = tempdir()?;
        let config_path = dir.path().join("config.json");
        
        // Create test settings
        let mut settings = Settings::default();
        settings.server_url = "https://test-server.com".to_string();
        settings.api_key = Some("test-api-key".to_string());
        settings.username = Some("test-user".to_string());
        
        // Save settings
        settings.save(&config_path)?;
        
        // Verify file exists
        assert!(config_path.exists());
        
        // Load settings
        let loaded = Settings::load(&config_path)?;
        
        // Verify loaded settings match what we saved
        assert_eq!(loaded.server_url, "https://test-server.com");
        assert_eq!(loaded.api_key, Some("test-api-key".to_string()));
        assert_eq!(loaded.username, Some("test-user".to_string()));
        assert_eq!(loaded.alsa_device, "default");
        
        Ok(())
    }
    
    #[test]
    fn test_settings_validation() {
        // Valid settings
        let valid_settings = Settings {
            server_url: "https://test-server.com".to_string(),
            api_key: Some("test-api-key".to_string()),
            username: None,
            alsa_device: "default".to_string(),
            user_id: None,
        };
        assert!(valid_settings.validate().is_ok());
        
        // Invalid settings - empty server URL
        let invalid_settings = Settings {
            server_url: "".to_string(),
            api_key: Some("test-api-key".to_string()),
            username: None,
            alsa_device: "default".to_string(),
            user_id: None,
        };
        assert!(invalid_settings.validate().is_err());
    }
    
    #[test]
    fn test_default_path() {
        let path = Settings::default_path();
        assert!(path.to_str().unwrap().contains(".config/jellycli/config.json"));
    }
}
