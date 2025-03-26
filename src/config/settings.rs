//! Application settings and configuration management

use serde::{Deserialize, Serialize};
use std::error::Error;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Application settings
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Settings {
    /// Jellyfin server URL
    pub server_url: String,
    /// API key for authentication (optional if using username/password)
    #[serde(default)]
    pub api_key: Option<String>,
    /// Username for Jellyfin login
    #[serde(default)]
    pub username: Option<String>,
    /// ALSA device to use for audio playback
    #[serde(default = "default_alsa_device")]
    pub alsa_device: String,
    /// User ID for Jellyfin requests
    #[serde(default)]
    pub user_id: Option<String>,
}

fn default_alsa_device() -> String {
    "default".to_string()
}

/// Error types for configuration operations
#[derive(Debug)]
pub enum ConfigError {
    IoError(io::Error),
    ParseError(String),
    ValidationError(String),
}

impl From<io::Error> for ConfigError {
    fn from(err: io::Error) -> Self {
        ConfigError::IoError(err)
    }
}

impl From<serde_json::Error> for ConfigError {
    fn from(err: serde_json::Error) -> Self {
        ConfigError::ParseError(err.to_string())
    }
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::IoError(e) => write!(f, "I/O error: {}", e),
            ConfigError::ParseError(s) => write!(f, "Parse error: {}", s),
            ConfigError::ValidationError(s) => write!(f, "Validation error: {}", s),
        }
    }
}

impl Error for ConfigError {}

impl Settings {
    /// Create default settings
    pub fn default() -> Self {
        Settings {
            server_url: "http://localhost:8096".to_string(),
            api_key: None,
            username: None,
            alsa_device: default_alsa_device(),
            user_id: None,
        }
    }

    /// Load settings from a file
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        if !path.exists() {
            return Ok(Self::default());
        }

        let content = fs::read_to_string(path)?;
        let settings: Settings = serde_json::from_str(&content)?;
        Ok(settings)
    }

    /// Save settings to a file
    pub fn save(&self, path: &Path) -> Result<(), ConfigError> {
        let content = serde_json::to_string_pretty(&self)?;
        
        // Create parent directories if they don't exist
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        
        fs::write(path, content)?;
        Ok(())
    }

    /// Get the default config file path
    pub fn default_path() -> PathBuf {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        home.join(".config").join("jellycli").join("config.json")
    }

    /// Validate settings
    pub fn validate(&self) -> Result<(), ConfigError> {
        // Check if server URL is valid
        if self.server_url.is_empty() {
            return Err(ConfigError::ValidationError("Server URL cannot be empty".to_string()));
        }

        // Check if we have either API key or username for authentication
        if self.api_key.is_none() && self.username.is_none() {
            return Err(ConfigError::ValidationError(
                "Either API key or username must be provided".to_string(),
            ));
        }

        Ok(())
    }
}
