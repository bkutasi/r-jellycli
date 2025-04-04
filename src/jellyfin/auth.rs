//! Jellyfin authentication implementation

use reqwest::{Client, header};
use std::error::Error;

use crate::jellyfin::models::{AuthRequest, AuthResponse};

/// Handles authentication with a Jellyfin server
pub async fn authenticate(
    client: &Client,
    server_url: &str,
    username: &str,
    password: &str,
) -> Result<AuthResponse, Box<dyn Error>> {
    // Normalize server URL by removing trailing slash if present
    let server_url = server_url.trim_end_matches('/');
    let auth_url = format!("{}/Users/authenticatebyname", server_url);
    
    
    // Prepare auth request payload
    let auth_request = AuthRequest {
        username: username.to_string(),
        pw: password.to_string(),
    };
    
    // Create required headers
    let mut headers = header::HeaderMap::new();
    headers.insert(
        "Content-Type", 
        header::HeaderValue::from_static("application/json")
    );
    headers.insert(
        "X-Emby-Authorization", 
        header::HeaderValue::from_static(
            "MediaBrowser Client=\"r-jellycli\", Device=\"HeadlessPlayer\", DeviceId=\"r-jellycli\", Version=\"0.1.0\", DeviceName=\"Jellyfin CLI Player\""
        )
    );
    
    
    // Send authentication request
    let response = client
        .post(&auth_url)
        .headers(headers)
        .json(&auth_request)
        .send()
        .await?;
        
    
    // Handle different response statuses
    match response.status() {
        reqwest::StatusCode::OK => {
            // Get the raw response text first for debugging
            let response_text = response.text().await?;
            
            // Parse the response manually
            match serde_json::from_str::<AuthResponse>(&response_text) {
                Ok(auth_response) => {
                    Ok(auth_response)
                },
                Err(e) => {
                    Err(format!("Failed to parse auth response: {}", e).into())
                }
            }
        }
        reqwest::StatusCode::BAD_REQUEST => {
            let error_text = response.text().await?;
            Err(format!("Login failed: {}", error_text).into())
        }
        _ => {
            let error_text = response.text().await?;
            Err(format!("Login failed: {}", error_text).into())
        }
    }
}

/// Creates an authorization header with token for authenticated requests
pub fn create_auth_header(token: &str) -> header::HeaderMap {
    let mut headers = header::HeaderMap::new();
    let token_value = header::HeaderValue::from_str(token)
        .expect("Failed to create header value from token");
    headers.insert("X-Emby-Token", token_value);
    headers
}
