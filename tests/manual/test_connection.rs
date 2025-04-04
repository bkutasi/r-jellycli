//! Test connection to Jellyfin server
//!
//! A diagnostic utility to test connectivity with a Jellyfin server
//! Run with: cargo run --bin test_connection -- --server-url http://your-server:8096 [--username user] [--password pass]

use std::error::Error;
use reqwest::Client;
use clap::Parser;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Jellyfin server URL
    #[arg(short, long)]
    server_url: String,
    
    /// Jellyfin username
    #[arg(short, long)]
    username: Option<String>,
    
    /// Jellyfin password
    #[arg(short, long)]
    password: Option<String>,
}

/// Connection test utility for Jellyfin server
struct ConnectionTester {
    client: Client,
    server_url: String,
    username: Option<String>,
    password: Option<String>,
}

impl ConnectionTester {
    /// Create a new connection tester
    fn new(args: Args) -> Self {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("Failed to create HTTP client");
            
        ConnectionTester {
            client,
            server_url: args.server_url,
            username: args.username,
            password: args.password,
        }
    }
    
    /// Test basic connectivity to server
    async fn test_basic_connectivity(&self) -> Result<(), Box<dyn Error>> {
        println!("\n1. Testing basic server connectivity...");
        match self.client.get(&self.server_url).send().await {
            Ok(response) => {
                println!("Server status: {}", response.status());
                println!("Response body: {}", response.text().await?);
            },
            Err(e) => {
                println!("❌ Connection failed: {}", e);
                println!("  - Check if the server is running");
                println!("  - Verify the URL is correct: {}", self.server_url);
                println!("  - Check your network connection");
            }
        }
        
        Ok(())
    }
    
    /// Test system info endpoint
    async fn test_system_info(&self) -> Result<(), Box<dyn Error>> {
        println!("\n2. Testing system info endpoint...");
        let system_url = format!("{}/System/Info", self.server_url);
        match self.client.get(&system_url).send().await {
            Ok(response) => {
                println!("Status: {}", response.status());
                println!("Response: {}", response.text().await?);
            },
            Err(e) => {
                println!("❌ System info request failed: {}", e);
                println!("  - The System/Info endpoint might be restricted");
                println!("  - Try the public endpoint instead");
            }
        }
        
        Ok(())
    }
    
    /// Test authentication endpoint
    async fn test_authentication(&self) -> Result<(), Box<dyn Error>> {
        if let (Some(username), Some(password)) = (self.username.as_ref(), self.password.as_ref()) {
            println!("\n3. Testing authentication...");
            let auth_url = format!("{}/Users/AuthenticateByName", self.server_url);
            
            #[derive(serde::Serialize, Debug)]
            struct AuthRequest {
                #[serde(rename = "Username")]
                username: String,
                #[serde(rename = "Pw")]
                pw: String,
            }
            
            let auth_request = AuthRequest {
                username: username.clone(),
                pw: password.clone(),
            };
            
            println!("Auth request URL: {}", auth_url);
            println!("Auth request payload: Username={}, Pw=[REDACTED]", username);
            
            println!("\n3.1 Testing authentication with json payload...");
            
            match self.client
                .post(&auth_url)
                .header("Content-Type", "application/json")
                .header("X-Emby-Authorization", 
                       "MediaBrowser Client=\"JellyfinCLI\", Device=\"CLI\", DeviceId=\"r-jellycli\", Version=\"0.1.0\"")
                .json(&auth_request)
                .send()
                .await {
                    
                Ok(resp) => {
                    println!("Auth status: {}", resp.status());
                    println!("Auth headers: {:?}", resp.headers());
                    let text = resp.text().await?;
                    println!("Auth response: {}", text);
                },
                Err(e) => {
                    println!("❌ Auth request error: {:?}", e);
                }
            }
            
            println!("\n3.2 Testing simple GET to auth URL...");
            match self.client.get(&auth_url).send().await {
                Ok(resp) => {
                    println!("GET status: {}", resp.status());
                    let text = resp.text().await?;
                    println!("GET response: {}", text);
                },
                Err(e) => {
                    println!("❌ GET request error: {:?}", e);
                }
            }
        } else {
            println!("\n3. Skipping authentication tests (no credentials provided)");
            println!("   To test authentication, provide --username and --password arguments");
        }
        
        Ok(())
    }
    
    /// Test public system info endpoint
    async fn test_public_info(&self) -> Result<(), Box<dyn Error>> {
        println!("\n4. Checking server info/version (public endpoint)...");
        let info_url = format!("{}/System/Info/Public", self.server_url);
        match self.client.get(&info_url).send().await {
            Ok(resp) => {
                println!("Info status: {}", resp.status());
                let text = resp.text().await?;
                println!("Server info: {}", text);
            },
            Err(e) => {
                println!("❌ Info request error: {:?}", e);
            }
        }
        
        Ok(())
    }
    
    /// Run all tests
    async fn run_all_tests(&self) -> Result<(), Box<dyn Error>> {
        self.test_basic_connectivity().await?;
        self.test_system_info().await?;
        self.test_authentication().await?;
        self.test_public_info().await?;
        
        println!("\n✅ All tests completed!");
        
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let args = Args::parse();
    
    let tester = ConnectionTester::new(args);
    tester.run_all_tests().await?;
    
    Ok(())
}
