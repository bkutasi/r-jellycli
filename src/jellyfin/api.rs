//! Jellyfin API client implementation

use reqwest::{Client, Error as ReqwestError};
use std::error::Error;
// Removed unused AuthRequest import
use crate::jellyfin::models::{MediaItem, ItemsResponse, AuthResponse};

/// Client for interacting with Jellyfin API
pub struct JellyfinClient {
    client: Client,
    server_url: String,
    api_key: Option<String>,
    user_id: Option<String>,
}

/// Error types for Jellyfin API operations
#[derive(Debug)]
pub enum JellyfinError {
    Network(ReqwestError),
    Authentication(&'static str),
    NotFound(&'static str),
    InvalidResponse(&'static str),
    Other(Box<dyn Error>),
}

impl From<ReqwestError> for JellyfinError {
    fn from(err: ReqwestError) -> Self {
        JellyfinError::Network(err)
    }
}

impl From<serde_json::Error> for JellyfinError {
    fn from(_err: serde_json::Error) -> Self {
        // Use InvalidResponse since we don't have a dedicated Parse variant
        JellyfinError::InvalidResponse("Failed to parse JSON response")
    }
}

impl std::fmt::Display for JellyfinError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JellyfinError::Network(e) => write!(f, "Network error: {}", e),
            JellyfinError::Authentication(msg) => write!(f, "Authentication error: {}", msg),
            JellyfinError::NotFound(msg) => write!(f, "Not found: {}", msg),
            JellyfinError::InvalidResponse(msg) => write!(f, "Invalid response: {}", msg),
            JellyfinError::Other(e) => write!(f, "Error: {}", e),
        }
    }
}

impl Error for JellyfinError {}

impl JellyfinClient {
    /// Create a new Jellyfin client with the server URL
    pub fn new(server_url: &str) -> Self {
        println!("[CLIENT-DEBUG] Creating new JellyfinClient with server_url: {}", server_url);
        
        // Create HTTP client with extended timeout, matching the auth_test configuration
        let client = match Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build() {
                Ok(client) => {
                    println!("[CLIENT-DEBUG] HTTP client created successfully with 30s timeout");
                    client
                },
                Err(e) => {
                    println!("[CLIENT-DEBUG] Error creating HTTP client with timeout: {:?}", e);
                    println!("[CLIENT-DEBUG] Falling back to default client");
                    Client::new()
                }
            };
            
        let normalized_url = server_url.trim_end_matches('/').to_string();
        println!("[CLIENT-DEBUG] Normalized server URL: {}", normalized_url);
        
        JellyfinClient {
            client,
            server_url: normalized_url,
            api_key: None,
            user_id: None,
        }
    }

    /// Set API key for authentication
    pub fn with_api_key(mut self, api_key: &str) -> Self {
        self.api_key = Some(api_key.to_string());
        self
    }

    /// Set user ID for requests
    pub fn with_user_id(mut self, user_id: &str) -> Self {
        self.user_id = Some(user_id.to_string());
        self
    }
    
    /// Get server URL (primarily for testing)
    pub fn get_server_url(&self) -> &str {
        &self.server_url
    }
    
    /// Get API key (primarily for testing)
    pub fn get_api_key(&self) -> &Option<String> {
        &self.api_key
    }
    
    /// Get user ID (primarily for testing)
    pub fn get_user_id(&self) -> &Option<String> {
        &self.user_id
    }
    
    /// Authenticate with Jellyfin using username and password
    pub async fn authenticate(&mut self, username: &str, password: &str) -> Result<AuthResponse, JellyfinError> {
        println!("[CLIENT-DEBUG] Authenticating with username: {}", username);
        println!("[CLIENT-DEBUG] Password length: {}", password.len());
        println!("[CLIENT-DEBUG] Server URL: {}", self.server_url);
        
        // Create a direct request using reqwest to test connectivity before calling authenticate
        println!("[CLIENT-DEBUG] Testing direct connectivity to server...");
        match self.client.get(&self.server_url).send().await {
            Ok(response) => {
                println!("[CLIENT-DEBUG] Direct connection test: {} {}", response.status(), response.status().as_str());
            },
            Err(e) => {
                println!("[CLIENT-DEBUG] Direct connection test failed: {:?}", e);
            }
        }
        
        // Use the auth module's authenticate function which is known to work
        println!("[CLIENT-DEBUG] Calling auth::authenticate function...");
        match crate::jellyfin::authenticate(&self.client, &self.server_url, username, password).await {
            Ok(auth_response) => {
                println!("[CLIENT-DEBUG] Authentication successful");
                // Store the auth info in our client
                self.api_key = Some(auth_response.access_token.clone());
                self.user_id = Some(auth_response.user.id.clone());
                Ok(auth_response)
            },
            Err(e) => {
                // Preserve the original error for better debugging
                println!("[CLIENT-DEBUG] Authentication error details: {:?}", e);
                Err(JellyfinError::Other(e))
            }
        }
    }

    /// Get root items from the user's library (Views)
    pub async fn get_items(&self) -> Result<Vec<MediaItem>, JellyfinError> {
        println!("[CLIENT-DEBUG] get_items (root views) called");

        // Ensure user_id and api_key are set
        let user_id = match self.user_id.as_deref() {
            Some(id) => id,
            None => {
                println!("[CLIENT-DEBUG] User ID not set before calling get_items");
                return Err(JellyfinError::Authentication("User ID not set after authentication"));
            }
        };
        let token = match self.api_key.as_deref() {
            Some(t) => t,
            None => {
                println!("[CLIENT-DEBUG] No API token available");
                return Err(JellyfinError::Authentication("API key not set"));
            }
        };

        println!("[CLIENT-DEBUG] Using user_id: {}", user_id);
        println!("[CLIENT-DEBUG] Using token: {}", token);

        // Use the /Users/{UserId}/Views endpoint to get the top-level library views
        let url = format!("{}/Users/{}/Views", self.server_url, user_id);
        println!("[CLIENT-DEBUG] Requesting URL: {}", url);

        // First get the raw response to inspect it
        let raw_response = self.client
            .get(&url)
            // No query parameter needed, user ID is in the path
            .header("X-Emby-Token", token)
            .send()
            .await?;

        println!("[CLIENT-DEBUG] Response status: {}", raw_response.status());

        // Get the response text to debug
        let response_text = raw_response.text().await?;
        println!("[CLIENT-DEBUG] Response text length: {} bytes", response_text.len());

        let response = if !response_text.is_empty() {
            println!("[CLIENT-DEBUG] First 100 chars: {}", &response_text[..std::cmp::min(100, response_text.len())]);
            // Try to parse it
            match serde_json::from_str::<ItemsResponse>(&response_text) {
                Ok(parsed) => parsed,
                Err(e) => {
                    // Log the full response text on parsing error for better debugging
                    println!("[CLIENT-DEBUG] JSON parsing error: {}. Full response text:\n{}", e, response_text);
                    return Err(JellyfinError::InvalidResponse("Failed to parse items response"));
                }
            }
        } else {
            println!("[CLIENT-DEBUG] Empty response received");
            return Err(JellyfinError::InvalidResponse("Empty response received"));
        };

        println!("[CLIENT-DEBUG] Successfully parsed response with {} items", response.items.len());
        Ok(response.items)
    }
    
    /// Get child items of a folder
    pub async fn get_items_by_parent_id(&self, parent_id: &str) -> Result<Vec<MediaItem>, JellyfinError> {
        let user_id = self.user_id.as_deref().unwrap_or("Default");
        let url = format!("{}/Users/{}/Items?ParentId={}", self.server_url, user_id, parent_id);

        let response = match &self.api_key {
            Some(token) => {
                self.client
                    .get(&url)
                    .header("X-Emby-Token", token)
                    .send()
                    .await?
                    .json::<ItemsResponse>()
                    .await?
            }
            None => return Err(JellyfinError::Authentication("API key not set")),
        };

        Ok(response.items)
    }
    
    /// Get streaming URL for an item
    pub fn get_stream_url(&self, item_id: &str) -> Result<String, JellyfinError> {
        // No need to use user_id for streaming URL
        
        match &self.api_key {
            Some(token) => {
                let url = format!("{}/Audio/{}/stream?static=true&api_key={}", 
                    self.server_url, item_id, token);
                Ok(url)
            }
            None => Err(JellyfinError::Authentication("API key not set")),
        }
    }
}
