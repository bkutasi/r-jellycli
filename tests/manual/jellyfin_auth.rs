//! Enhanced Jellyfin authentication test utility
//!
//! This utility tests authentication with detailed diagnostics
//! Run with: cargo run --bin jellyfin_auth

use std::error::Error;
use reqwest::{Client, header};
use std::fs;
use std::time::Duration;
use serde_json;
use std::env;
use std::path::Path;

#[path = "../test_utils.rs"]
mod test_utils;
use test_utils::Credentials;

/// Run the authentication test with detailed diagnostics
struct AuthenticationTester {
    client: Client,
    credentials: Credentials,
    device_id: String,
    client_name: String,
    client_version: String,
}

impl AuthenticationTester {
    /// Create a new authentication tester with detailed diagnostics
    fn new(credentials_path: &Path) -> Result<Self, Box<dyn Error>> {
        // Load credentials
        println!("Loading credentials from {}...", credentials_path.display());
        let credentials = test_utils::load_credentials(credentials_path)?;
        
        // Create HTTP client with advanced options
        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .connect_timeout(Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
            
        Ok(AuthenticationTester {
            client,
            credentials,
            device_id: "jellycli-rust".to_string(),
            client_name: "Jellyfin Rust CLI".to_string(),
            client_version: "0.1.0".to_string(),
        })
    }
    
    /// Get properly formatted server URL
    fn get_server_url(&self) -> String {
        // Determine if we should try with HTTPS (can be set via environment variable)
        let use_https = env::var("USE_HTTPS").unwrap_or_else(|_| "false".to_string()) == "true";
        
        // Try with original URL first
        let server_url = self.credentials.server_url.trim_end_matches('/');
        
        // If server URL doesn't start with http:// or https://, prepend http:// or https://
        if !server_url.starts_with("http://") && !server_url.starts_with("https://") {
            if use_https {
                format!("https://{}", server_url)
            } else {
                format!("http://{}", server_url)
            }
        } else {
            server_url.to_string()
        }
    }
    
    /// Create authentication headers
    fn create_auth_headers(&self) -> Result<header::HeaderMap, Box<dyn Error>> {
        // Create base headers
        let mut headers = header::HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE, 
            header::HeaderValue::from_static("application/json")
        );
        
        // Create the main authorization header
        let auth_header = format!(
            "MediaBrowser Client=\"{}\", Device=\"CLI\", DeviceId=\"{}\", Version=\"{}\"",
            self.client_name, self.device_id, self.client_version
        );
        
        println!("Authorization header: {}", auth_header);
        
        // Try with X-Emby-Authorization as recommended in the guide
        headers.insert(
            "X-Emby-Authorization", 
            header::HeaderValue::from_str(&auth_header)?
        );
        
        // Add User-Agent for good measure
        headers.insert(
            header::USER_AGENT, 
            header::HeaderValue::from_str(&format!("{}/{}", self.client_name, self.client_version))?
        );
        
        Ok(headers)
    }
    
    /// Test authentication against the Jellyfin server
    async fn test_authentication(&self) -> Result<(), Box<dyn Error>> {
        let server_url = self.get_server_url();
        let auth_url = format!("{}/Users/authenticatebyname", server_url);
        
        println!("\n📡 Connection Information:");
        println!("Server URL: {}", server_url);
        println!("Authentication URL: {}", auth_url);
        println!("Username: {}", self.credentials.username);
        
        println!("\n📋 Authentication Request Details:");
        
        // Create authentication body
        let auth_body = serde_json::json!({
            "Username": self.credentials.username,
            "Pw": self.credentials.password
        });
        
        // Create headers
        let headers = self.create_auth_headers()?;
        
        println!("\n🔄 Sending authentication request...");
        println!("Full URL: {}", auth_url);
        println!("Method: POST");
        println!("Headers: {:#?}", headers);
        
        // Log the complete request with password redacted
        println!("Request body: {}", serde_json::to_string_pretty(&auth_body)?.replace(&self.credentials.password, "[REDACTED]"));
        
        // Make the actual request
        let response = self.client
            .post(&auth_url)
            .headers(headers.clone())
            .json(&auth_body)
            .send()
            .await?;
        
        println!("\n📊 Response Information:");
        println!("Status: {}", response.status());
        println!("Response headers: {:#?}", response.headers());
        
        // Process the response
        self.process_response(response, server_url).await?;
        
        Ok(())
    }
    
    /// Process the authentication response
    async fn process_response(&self, response: reqwest::Response, server_url: String) -> Result<(), Box<dyn Error>> {
        // Handle response based on status code
        match response.status() {
            reqwest::StatusCode::OK => {
                println!("\n✅ Authentication successful!");
                let json_response: serde_json::Value = response.json().await?;
                
                if let Some(token) = json_response.get("AccessToken") {
                    println!("Access token received! (not displayed for security)");
                    println!("Token length: {}", token.as_str().unwrap_or_default().len());
                    
                    // Save token to a file for future use
                    let token_info = serde_json::json!({
                        "server": server_url,
                        "user_id": json_response.get("User").and_then(|u| u.get("Id")).and_then(|id| id.as_str()).unwrap_or_default(),
                        "token": token.as_str().unwrap_or_default(),
                        "device_id": self.device_id
                    });
                    
                    fs::write("jellyfin_token.json", serde_json::to_string_pretty(&token_info)?)?;
                    println!("Token saved to jellyfin_token.json");
                } else {
                    println!("No access token found in response.");
                }
                
                if let Some(user) = json_response.get("User") {
                    println!("User information: {}", user);
                }
            },
            reqwest::StatusCode::FOUND | reqwest::StatusCode::MOVED_PERMANENTLY | 
            reqwest::StatusCode::TEMPORARY_REDIRECT | reqwest::StatusCode::PERMANENT_REDIRECT => {
                // Handle redirects manually since we disabled automatic redirects
                if let Some(location) = response.headers().get(reqwest::header::LOCATION) {
                    println!("\n🔄 Server redirected us to: {}", location.to_str()?);
                    println!("Try using this URL instead in your credentials.json file.");
                } else {
                    println!("\n🔄 Server returned a redirect but didn't provide a Location header");
                }
                let body = response.text().await?;
                println!("Redirect response body: {}", body);
            },
            _ => {
                println!("\n❌ Authentication failed with status: {}", response.status());
                
                let error_text = response.text().await?;
                println!("Error response body: {}", error_text);
                
                println!("\n🔍 Troubleshooting Steps:");
                println!("1. Verify server URL: {} (try adding/removing trailing slash)", server_url);
                println!("2. Check if your username is correct: {}", self.credentials.username);
                println!("3. Verify your password is correct");
                println!("4. Try HTTPS: Run with environment variable USE_HTTPS=true");
                println!("5. Check Jellyfin server logs for more information");
                println!("6. Verify Jellyfin server allows remote connections");
                println!("7. Check if your Jellyfin API version is compatible");
            }
        }
        
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    println!("\n🔐 Jellyfin Authentication Test");
    
    // Default credentials path
    let credentials_path = Path::new("credentials.json");
    
    // Create tester and run test
    let tester = AuthenticationTester::new(credentials_path)?;
    tester.test_authentication().await?;
    
    Ok(())
}
