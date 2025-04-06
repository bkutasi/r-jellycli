// tests/integration/jellyfin_api_sequence_test.rs

#![allow(unused_imports)] // Allow while building

use r_jellycli::jellyfin::api::{JellyfinClient, JellyfinError};
use r_jellycli::jellyfin::models::{AuthResponse, MediaItem}; // Assuming models are here
use r_jellycli::jellyfin::CapabilitiesReport; // Assuming CapabilitiesReport is here
use std::env;
use reqwest::StatusCode; // Keep for potential direct checks if needed, but prefer client methods
use serde_json::Value;

// Helper function to load credentials from environment variables
// Panics if variables are not set or empty.
fn load_test_credentials() -> (String, String, String) {
    dotenv::dotenv().ok(); // Load .env file if present

    let url = env::var("JELLYFIN_TEST_URL")
        .expect("JELLYFIN_TEST_URL environment variable not set. Needed for integration tests.");
    let user = env::var("JELLYFIN_TEST_USER")
        .expect("JELLYFIN_TEST_USER environment variable not set. Needed for integration tests.");
    let password = env::var("JELLYFIN_TEST_PASSWORD")
        .expect("JELLYFIN_TEST_PASSWORD environment variable not set. Needed for integration tests.");

    if url.is_empty() { panic!("JELLYFIN_TEST_URL environment variable is empty."); }
    if user.is_empty() { panic!("JELLYFIN_TEST_USER environment variable is empty."); }
    if password.is_empty() { panic!("JELLYFIN_TEST_PASSWORD environment variable is empty."); }
    // Ensure the URL starts with a valid protocol
    if !url.starts_with("http://") && !url.starts_with("https://") {
        panic!("JELLYFIN_TEST_URL must start with http:// or https://. Found: {}", url);
    }

    (url, user, password)
}

#[tokio::test]
#[ignore] // Requires a running Jellyfin instance and credentials set in environment
async fn test_jellyfin_api_sequence_with_client() {
    // --- Setup ---
    println!("--- Running Jellyfin API Sequence Integration Test (using JellyfinClient) ---");
    println!("Ensure a Jellyfin server is running and accessible via JELLYFIN_TEST_URL.");
    println!("Required environment variables (or .env file):");
    println!("  JELLYFIN_TEST_URL=http://your-jellyfin-url:8096");
    println!("  JELLYFIN_TEST_USER=your_test_username");
    println!("  JELLYFIN_TEST_PASSWORD=your_test_password");

    let (server_url, username, password) = load_test_credentials();

    // Use the actual JellyfinClient
    let mut client = JellyfinClient::new(&server_url);

    // --- 1. Authenticate ---
    println!("Step 1: Authenticating using client...");
    let auth_result = client.authenticate(&username, &password).await;

    assert!(auth_result.is_ok(), "Authentication failed: {:?}", auth_result.err());
    let auth_response = auth_result.unwrap();
    println!("Authentication successful. User ID: {}", auth_response.user.id);
    assert!(!auth_response.access_token.is_empty(), "AccessToken is empty");
    assert!(!auth_response.user.id.is_empty(), "UserId is empty");
    // Client now stores the token and user_id internally

    // --- 2. Report Capabilities ---
    println!("Step 2: Reporting Capabilities using client...");
    // Construct the capabilities report based on the structure expected by the client method
    let capabilities_report = CapabilitiesReport {
         playable_media_types: vec!["Audio".to_string()],
         supported_commands: vec![
             "PlayState".to_string(), "Play".to_string(), "Seek".to_string(),
             "NextTrack".to_string(), "PreviousTrack".to_string(),
             "SetShuffleQueue".to_string(), "SetRepeatMode".to_string(), "PlayNext".to_string()
         ],
         supports_media_control: true,
         supports_persistent_identifier: false,
         // Add other fields if the struct requires them (e.g., device_profile, icon_url etc. might be Option<String>)
         // Assuming these are optional or handled by the client method if not provided explicitly here.
         // Check the definition of CapabilitiesReport if this step fails.
         device_profile: None,
         app_store_url: None,
         icon_url: None,
    };

    let cap_result = client.report_capabilities(&capabilities_report).await;
    assert!(cap_result.is_ok(), "Reporting capabilities failed: {:?}", cap_result.err());
    println!("Capabilities reported successfully.");


    // --- 3. Fetch Views ---
    println!("Step 3: Fetching Views using client...");
    let views_result = client.get_items().await; // get_items() fetches Views

    assert!(views_result.is_ok(), "Fetching views failed: {:?}", views_result.err());
    let views = views_result.unwrap();
    println!("Views fetched successfully. Count: {}", views.len());
    // Optional: Assert views are not empty if expected
    // assert!(!views.is_empty(), "Views list should not be empty for a typical setup");


    // --- 4. Fetch Items (Root) ---
    // This method doesn't exist yet. Writing the test first (TDD).
    // This will cause a compilation error until implemented in JellyfinClient.
    println!("Step 4: Fetching Root Items using client (expecting get_root_items method)...");
    let root_items_result = client.get_root_items().await; // Method to be added

    assert!(root_items_result.is_ok(), "Fetching root items failed: {:?}", root_items_result.err());
    let root_items = root_items_result.unwrap();
    println!("Root items fetched successfully. Count: {}", root_items.len());
    // Optional: Assert root items are not empty if expected
    // assert!(!root_items.is_empty(), "Root items list should not be empty for a typical setup");


    println!("--- Jellyfin API Sequence Test Completed Successfully (using JellyfinClient) ---");
}