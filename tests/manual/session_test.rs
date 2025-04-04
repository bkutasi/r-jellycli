//! Manual test for Jellyfin session management

use reqwest::Client;
use std::env;
use std::error::Error;
use std::time::Duration;
use tokio::sync::broadcast;
use tokio::time::sleep;

use uuid::Uuid;
use r_jellycli::jellyfin::{JellyfinClient, authenticate};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let server_url = env::var("JELLYFIN_SERVER_URL")
        .expect("JELLYFIN_SERVER_URL must be set");
    let username = env::var("JELLYFIN_USERNAME")
        .expect("JELLYFIN_USERNAME must be set");
    let password = env::var("JELLYFIN_PASSWORD")
        .expect("JELLYFIN_PASSWORD must be set");

    println!("==== Jellyfin Session Test ====");
    println!("Server URL: {}", server_url);
    println!("Username: {}", username);
    println!("Password: [redacted] ({} chars)", password.len());

    let client = Client::builder()
        .timeout(Duration::from_secs(30))
        .build()?;

    println!("\n[TEST] Step 1: Testing direct authentication...");
    let auth_response = match authenticate(&client, &server_url, &username, &password).await {
        Ok(resp) => {
            println!("[TEST] Authentication successful!");
            println!("[TEST] User ID: {}", resp.user.id);
            println!("[TEST] Access token length: {}", resp.access_token.len());
            resp
        }
        Err(e) => {
            println!("[TEST] Authentication error: {:?}", e);
            return Err(e);
        }
    };

    println!("\n[TEST] Step 2: Testing session initialization...");
    let mut jellyfin_client = JellyfinClient::new(&server_url)
        .with_api_key(&auth_response.access_token)
        .with_user_id(&auth_response.user.id);

    let test_device_id = Uuid::new_v4().to_string();
    println!("[TEST] Using dummy Device ID: {}", test_device_id);
    let (shutdown_tx, _shutdown_rx) = broadcast::channel::<()>(1);
    match jellyfin_client.initialize_session(&test_device_id, shutdown_tx).await {
        Ok(()) => {
            println!("[TEST] Session initialized successfully!");
        }
        Err(e) => {
            println!("[TEST] Session initialization error: {:?}", e);
            return Err(Box::new(e));
        }
    }

    println!("\n[TEST] Step 3: Session is active. Waiting for 2 minutes to monitor session pings...");
    println!("[TEST] Your Jellyfin client should now show this as an active session.");
    println!("[TEST] Check the Jellyfin dashboard to verify this client appears.");
    
    for i in 0..4 {
        sleep(Duration::from_secs(30)).await;
        println!("[TEST] Still alive... {} seconds elapsed", (i + 1) * 30);
    }

    println!("\n[TEST] Test completed successfully!");
    Ok(())
}
