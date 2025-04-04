//! Common utilities for testing Jellyfin CLI client
//! 
//! This module provides shared functionality across all test types.

use serde::{Deserialize, Serialize};
use std::error::Error;
use std::fs;
use std::path::Path;

/// Credentials structure for authentication tests
#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct Credentials {
    pub username: String,
    pub password: String,
    pub server_url: String,
}

/// Loads credentials from a JSON file for testing
pub fn load_credentials<P: AsRef<Path>>(path: P) -> Result<Credentials, Box<dyn Error>> {
    let creds_json = fs::read_to_string(path)?;
    let creds: Credentials = serde_json::from_str(&creds_json)?;
    Ok(creds)
}

#[cfg(test)]
    #[allow(dead_code)]
pub mod mocks {
    use reqwest::Client;
    use std::time::Duration;

    /// Create a test HTTP client with extended timeout
    pub fn create_test_client() -> Client {
        Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .expect("Failed to create test HTTP client")
    }
}

#[cfg(test)]
    #[allow(dead_code)]
pub mod constants {
    /// Default test server URL
    pub const TEST_SERVER_URL: &str = "http://localhost:8096";
    /// Default test API key
    pub const TEST_API_KEY: &str = "test_api_key_123456";
    /// Default test user ID
    pub const TEST_USER_ID: &str = "test_user_id_123456";
}
