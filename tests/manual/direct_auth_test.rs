//! Simple test utility for direct Jellyfin authentication
//!
//! This utility can be used to test authentication with a Jellyfin server
//! using command-line arguments directly like the main application.
//! Run with: cargo run --bin direct_auth_test -- --server-url "http://server:port" --username "user" --password "pass"

use clap::Parser;
use r_jellycli::jellyfin::authenticate;
use std::error::Error;
use std::time::Duration;

#[derive(Parser, Debug)]
#[clap(about = "Test Jellyfin authentication with direct arguments")]
struct Args {
    /// Jellyfin server URL
    #[clap(long, required = true)]
    server_url: String,
    
    /// Jellyfin username
    #[clap(long, required = true)]
    username: String,
    
    /// Jellyfin password
    #[clap(long, required = true)]
    password: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    // Parse command-line arguments
    let args = Args::parse();
    
    // Display input information (without showing password)
    println!("Server URL: {}", args.server_url);
    println!("Username: {}", args.username);
    println!("Password length: {}", args.password.len());
    println!("Raw password bytes: {:?}", args.password.as_bytes());
    
    // Create HTTP client with extended timeout
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()?;
    
    // Test authentication using the exact same function as auth_test
    println!("\nTesting authentication to Jellyfin server...");
    match authenticate(&client, &args.server_url, &args.username, &args.password).await {
        Ok(auth_response) => {
            println!("Authentication successful!");
            println!("Access token: {}", auth_response.access_token);
            println!("User ID: {}", auth_response.user.id);
            println!("Server ID: {}", auth_response.server_id);
        },
        Err(e) => {
            println!("Authentication failed: {}", e);
        }
    }
    
    Ok(())
}
