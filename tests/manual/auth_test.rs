//! Manual test utility for Jellyfin authentication
//!
//! This utility can be used to test authentication with a Jellyfin server.
//! Run with: cargo run --bin auth_test

// We need to access the crate through the crate name 'r_jellycli'
use r_jellycli::jellyfin::authenticate;
use std::error::Error;
use std::path::Path;
use std::time::Duration;

#[path = "../test_utils.rs"]
mod test_utils;
use test_utils::Credentials;

struct AuthTester {
    client: reqwest::Client,
    credentials: Credentials,
}

impl AuthTester {
    /// Create a new authentication tester
    fn new(credentials_path: &Path) -> Result<Self, Box<dyn Error>> {
        // Load credentials
        println!("Loading credentials from {}...", credentials_path.display());
        let credentials = test_utils::load_credentials(credentials_path)?;
        
        // Create HTTP client with extended timeout
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()?;
            
        Ok(AuthTester {
            client,
            credentials,
        })
    }
    
    /// Display credentials info (without showing password)
    fn display_credentials_info(&self) {
        println!("Server URL: {}", self.credentials.server_url);
        println!("Username: {}", self.credentials.username);
        println!("Password length: {}", self.credentials.password.len());
    }
    
    /// Test authentication with standard endpoint
    async fn test_standard_auth(&self) -> Result<Option<String>, Box<dyn Error>> {
        println!("\nTesting authentication to Jellyfin server...");
        
        match authenticate(
            &self.client, 
            &self.credentials.server_url, 
            &self.credentials.username, 
            &self.credentials.password
        ).await {
            Ok(auth_response) => {
                println!("Authentication successful!");
                println!("Access token: {}", auth_response.access_token);
                println!("User ID: {}", auth_response.user.id);
                println!("Server ID: {}", auth_response.server_id);
                
                Ok(Some(auth_response.access_token))
            },
            Err(e) => {
                println!("Authentication failed: {}", e);
                Ok(None)
            }
        }
    }
    
    /// Test authentication with alternative endpoint
    async fn test_alternative_auth(&self) -> Result<Option<String>, Box<dyn Error>> {
        println!("\nTrying alternative endpoint format...");
        
        // Try the alternative endpoint format
        let alt_server_url = if self.credentials.server_url.ends_with("/emby") {
            self.credentials.server_url.clone()
        } else {
            format!("{}/emby", self.credentials.server_url)
        };
        
        match authenticate(
            &self.client, 
            &alt_server_url, 
            &self.credentials.username, 
            &self.credentials.password
        ).await {
            Ok(auth_response) => {
                println!("Authentication successful with alternative endpoint!");
                println!("Access token: {}", auth_response.access_token);
                println!("User ID: {}", auth_response.user.id);
                
                Ok(Some(auth_response.access_token))
            },
            Err(e) => {
                println!("Alternative authentication failed: {}", e);
                Ok(None)
            }
        }
    }
    
    /// Test user data fetch with token
    async fn test_user_info(&self, token: &str) -> Result<(), Box<dyn Error>> {
        // Get user ID from the credentials (not ideal, but works for the test)
        // In a real implementation, we'd extract this from the token
        let user_info_url = format!("{}/Users/{}", 
                                  self.credentials.server_url,
                                  "Me");
                                  
        println!("\nTesting authenticated request to {}", user_info_url);
        
        let user_response = self.client
            .get(&user_info_url)
            .header("X-Emby-Token", token)
            .send()
            .await?;
            
        println!("Response status: {}", user_response.status());
        
        if user_response.status().is_success() {
            let body = user_response.text().await?;
            println!("User info: {}", body);
        } else {
            println!("Failed to fetch user info: {}", user_response.status());
        }
        
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    // Default credentials path
    let credentials_path = Path::new("credentials.json");
    
    // Create auth tester
    let tester = AuthTester::new(credentials_path)?;
    tester.display_credentials_info();
    
    // Try standard authentication
    if let Some(token) = tester.test_standard_auth().await? {
        // If we have a token, test user info
        tester.test_user_info(&token).await?;
    } else {
        // Try alternative endpoint
        if let Some(token) = tester.test_alternative_auth().await? {
            tester.test_user_info(&token).await?;
        } else {
            println!("Both authentication attempts failed. Please check your credentials and server URL.");
        }
    }
    
    Ok(())
}
