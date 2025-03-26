//! Integration tests for Jellyfin API client
//! 
//! These tests require a running Jellyfin server and are marked with #[ignore]
//! To run these tests: cargo test --test jellyfin_api_integration -- --ignored

use r_jellycli::jellyfin::JellyfinClient;
use std::env;

/// Setup a test client from environment variables
/// 
/// Requires:
/// - JELLYFIN_TEST_URL: URL of the Jellyfin server
/// - JELLYFIN_TEST_USERNAME: Username for authentication
/// - JELLYFIN_TEST_PASSWORD: Password for authentication
async fn setup_test_client() -> JellyfinClient {
    let server_url = env::var("JELLYFIN_TEST_URL")
        .expect("JELLYFIN_TEST_URL must be set for integration tests");
    
    JellyfinClient::new(&server_url)
}

#[tokio::test]
#[ignore] // Integration test requiring a running server
async fn test_authentication() {
    // This test requires a running Jellyfin server with valid credentials
    // Skip if environment variables are not configured
    if env::var("JELLYFIN_TEST_URL").is_err() ||
       env::var("JELLYFIN_TEST_USERNAME").is_err() ||
       env::var("JELLYFIN_TEST_PASSWORD").is_err() {
        eprintln!("Skipping integration test: environment variables not set");
        return;
    }
    
    let mut client = setup_test_client().await;
    let username = env::var("JELLYFIN_TEST_USERNAME").unwrap();
    let password = env::var("JELLYFIN_TEST_PASSWORD").unwrap();
    
    let result = client.authenticate(&username, &password).await;
    assert!(result.is_ok(), "Authentication failed: {:?}", result.err());
    
    let auth_response = result.unwrap();
    assert!(!auth_response.access_token.is_empty(), "Access token should not be empty");
    assert!(!auth_response.user.id.is_empty(), "User ID should not be empty");
    assert!(!auth_response.user.name.is_empty(), "Username should not be empty");
}

#[tokio::test]
#[ignore] // Integration test requiring a running server
async fn test_get_library_items() {
    // Skip if environment variables are not configured
    if env::var("JELLYFIN_TEST_URL").is_err() ||
       env::var("JELLYFIN_TEST_USERNAME").is_err() ||
       env::var("JELLYFIN_TEST_PASSWORD").is_err() {
        eprintln!("Skipping integration test: environment variables not set");
        return;
    }
    
    let mut client = setup_test_client().await;
    let username = env::var("JELLYFIN_TEST_USERNAME").unwrap();
    let password = env::var("JELLYFIN_TEST_PASSWORD").unwrap();
    
    // First authenticate
    let auth_result = client.authenticate(&username, &password).await;
    assert!(auth_result.is_ok(), "Authentication failed: {:?}", auth_result.err());
    
    // Then get library items
    let items_result = client.get_items().await;
    
    assert!(items_result.is_ok(), "Failed to get library items: {:?}", items_result.err());
    
    let items = items_result.unwrap();
    // Just test that we got some items - the actual content will depend on the test server
    println!("Found {} library items", items.len());
}

#[tokio::test]
#[ignore] // Integration test requiring a running server
async fn test_get_folder_items() {
    // Skip if environment variables are not configured
    if env::var("JELLYFIN_TEST_URL").is_err() ||
       env::var("JELLYFIN_TEST_USERNAME").is_err() ||
       env::var("JELLYFIN_TEST_PASSWORD").is_err() {
        eprintln!("Skipping integration test: environment variables not set");
        return;
    }
    
    let mut client = setup_test_client().await;
    let username = env::var("JELLYFIN_TEST_USERNAME").unwrap();
    let password = env::var("JELLYFIN_TEST_PASSWORD").unwrap();
    
    // First authenticate
    let auth_result = client.authenticate(&username, &password).await;
    assert!(auth_result.is_ok(), "Authentication failed: {:?}", auth_result.err());
    
    // Then get library items to find a folder
    let items_result = client.get_items().await;
    assert!(items_result.is_ok(), "Failed to get library items");
    
    let items = items_result.unwrap();
    
    // Try to find a folder item
    if let Some(folder) = items.iter().find(|item| item.is_folder) {
        // Get items inside the folder
        let folder_items_result = client.get_items_by_parent_id(&folder.id).await;
        assert!(folder_items_result.is_ok(), "Failed to get folder items: {:?}", folder_items_result.err());
        
        let folder_items = folder_items_result.unwrap();
        println!("Found {} items in folder '{}'", folder_items.len(), folder.name);
    } else {
        println!("No folders found in the library, skipping folder items test");
    }
}

#[tokio::test]
#[ignore] // Integration test requiring a running server
async fn test_get_stream_url() {
    // Skip if environment variables are not configured
    if env::var("JELLYFIN_TEST_URL").is_err() ||
       env::var("JELLYFIN_TEST_USERNAME").is_err() ||
       env::var("JELLYFIN_TEST_PASSWORD").is_err() {
        eprintln!("Skipping integration test: environment variables not set");
        return;
    }
    
    let mut client = setup_test_client().await;
    let username = env::var("JELLYFIN_TEST_USERNAME").unwrap();
    let password = env::var("JELLYFIN_TEST_PASSWORD").unwrap();
    
    // First authenticate
    let auth_result = client.authenticate(&username, &password).await;
    assert!(auth_result.is_ok(), "Authentication failed: {:?}", auth_result.err());
    
    // Then get library items to find a media item
    let items_result = client.get_items().await;
    assert!(items_result.is_ok(), "Failed to get library items");
    
    let items = items_result.unwrap();
    
    // Try to find a non-folder item (likely a media item)
    if let Some(media_item) = items.iter().find(|item| !item.is_folder) {
        // Get stream URL for the media item
        let stream_url_result = client.get_stream_url(&media_item.id);
        assert!(stream_url_result.is_ok(), "Failed to get stream URL: {:?}", stream_url_result.err());
        
        // Verify the URL contains the expected parts
        let stream_url = stream_url_result.unwrap();
        assert!(stream_url.contains(client.get_server_url()), "Stream URL should contain server URL");
        assert!(stream_url.contains(&media_item.id), "Stream URL should contain item ID");
        assert!(stream_url.contains("api_key="), "Stream URL should contain API key");
        
        println!("Successfully generated stream URL for '{}'", media_item.name);
    } else {
        println!("No media items found in the library, skipping stream URL test");
    }
}
