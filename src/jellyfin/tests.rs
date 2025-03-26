//! Unit tests for Jellyfin API client

#[cfg(test)]
mod tests {
    use crate::jellyfin::JellyfinClient;
    
    #[test]
    fn test_client_creation() {
        let client = JellyfinClient::new("http://localhost:8096");
        assert_eq!(client.get_server_url(), "http://localhost:8096");
        assert!(client.get_api_key().is_none());
        assert!(client.get_user_id().is_none());
    }
    
    #[test]
    fn test_client_with_api_key() {
        let client = JellyfinClient::new("http://localhost:8096")
            .with_api_key("test_api_key");
        assert_eq!(client.get_server_url(), "http://localhost:8096");
        assert_eq!(*client.get_api_key(), Some("test_api_key".to_string()));
        assert!(client.get_user_id().is_none());
    }
    
    #[test]
    fn test_client_with_user_id() {
        let client = JellyfinClient::new("http://localhost:8096")
            .with_api_key("test_api_key")
            .with_user_id("test_user_id");
        assert_eq!(client.get_server_url(), "http://localhost:8096");
        assert_eq!(*client.get_api_key(), Some("test_api_key".to_string()));
        assert_eq!(*client.get_user_id(), Some("test_user_id".to_string()));
    }
    
    #[test]
    fn test_get_stream_url() {
        let client = JellyfinClient::new("http://localhost:8096")
            .with_api_key("test_api_key");
        let url = client.get_stream_url("item123").unwrap();
        assert_eq!(url, "http://localhost:8096/Audio/item123/stream?static=true&api_key=test_api_key");
    }
    
    // Integration tests that require a Jellyfin server would go here
    // These would be marked with #[ignore] to avoid running them in regular test runs
    // For example:
    // #[tokio::test]
    // #[ignore]
    // async fn test_authenticate() {...}
}
