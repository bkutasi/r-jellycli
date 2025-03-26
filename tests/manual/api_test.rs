//! Manual test utility for Jellyfin API endpoints
//!
//! This utility can be used to test various Jellyfin API endpoints
//! Run with: cargo run --bin api_test

use reqwest::{Client, Response};
use std::error::Error;
use std::path::Path;
use std::time::Duration;

#[path = "../test_utils.rs"]
mod test_utils;
use test_utils::Credentials;

/// API test harness
struct ApiTester {
    client: Client,
    credentials: Credentials,
}

impl ApiTester {
    /// Create a new API tester
    fn new(credentials_path: &Path) -> Result<Self, Box<dyn Error>> {
        // Load credentials
        println!("Loading credentials from {}...", credentials_path.display());
        let credentials = test_utils::load_credentials(credentials_path)?;
        
        // Create HTTP client with extended timeout
        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .build()?;
            
        Ok(ApiTester {
            client,
            credentials,
        })
    }
    
    /// Test server info endpoint (public, no auth required)
    async fn test_server_info(&self) -> Result<(), Box<dyn Error>> {
        println!("\n1. Checking server info (no auth required)...");
        let info_url = format!("{}/System/Info/Public", self.credentials.server_url);
        let response = self.client.get(&info_url).send().await?;
        
        print_response(response).await?;
        Ok(())
    }
    
    /// Test authentication with manual JSON payload
    async fn test_authentication(&self) -> Result<(), Box<dyn Error>> {
        println!("\n2. Testing basic authentication...");
        let auth_url = format!("{}/Users/authenticatebyname", self.credentials.server_url);
        
        // Manually build the JSON payload to ensure it's properly formatted
        let auth_body = format!(
            r#"{{"Username":"{}","Pw":"{}"}}"#, 
            self.credentials.username, 
            self.credentials.password
        );
        
        println!("URL: {}", auth_url);
        println!("Request body: {}", auth_body);
        
        let response = self.client
            .post(&auth_url)
            .header("Content-Type", "application/json")
            .body(auth_body)
            .send()
            .await?;
            
        print_response(response).await?;
        Ok(())
    }
    
    /// Test authentication with lowercase URL
    async fn test_lowercase_auth(&self) -> Result<(), Box<dyn Error>> {
        println!("\n3. Testing with lowercase URL...");
        let auth_url = format!("{}/users/authenticatebyname", self.credentials.server_url);
        let auth_body = format!(
            r#"{{"Username":"{}","Pw":"{}"}}"#, 
            self.credentials.username, 
            self.credentials.password
        );
        
        let response = self.client
            .post(&auth_url)
            .header("Content-Type", "application/json")
            .body(auth_body)
            .send()
            .await?;
            
        print_response(response).await?;
        Ok(())
    }
    
    /// Test authentication with alternate URL structure
    async fn test_alternate_auth(&self) -> Result<(), Box<dyn Error>> {
        println!("\n4. Testing alternate URL structure...");
        let auth_url = format!("{}/emby/Users/AuthenticateByName", self.credentials.server_url);
        let auth_body = format!(
            r#"{{"Username":"{}","Pw":"{}"}}"#, 
            self.credentials.username, 
            self.credentials.password
        );
        
        let response = self.client
            .post(&auth_url)
            .header("Content-Type", "application/json")
            .body(auth_body)
            .send()
            .await?;
            
        print_response(response).await?;
        Ok(())
    }
}

/// Print a reqwest::Response in a formatted way
async fn print_response(response: Response) -> Result<(), Box<dyn Error>> {
    println!("Status: {}", response.status());
    println!("Headers: {:?}", response.headers());
    
    let body = response.text().await?;
    println!("Body: {}", body);
    
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    // Default credentials path
    let credentials_path = Path::new("credentials.json");
    
    // Create API tester
    let tester = ApiTester::new(credentials_path)?;
    
    // Run tests
    tester.test_server_info().await?;
    tester.test_authentication().await?;
    tester.test_lowercase_auth().await?;
    tester.test_alternate_auth().await?;
    
    Ok(())
}
