//! Tests for the command-line interface

#[cfg(test)]
mod tests {
    use super::super::*;
    use crate::jellyfin::MediaItem;
    
    
    
    #[test]
    fn test_args_parsing() {
        use clap::CommandFactory;
        let app = Args::command();
        app.debug_assert();
    }
    
    #[test]
    fn test_args_defaults() {
        let args = Args {
            server_url: None,
            api_key: None,
            username: None,
            password: None,
            alsa_device: "default".to_string(),
            config: None,
        };
        
        assert_eq!(args.alsa_device, "default");
        assert!(args.server_url.is_none());
        assert!(args.api_key.is_none());
        assert!(args.username.is_none());
        assert!(args.password.is_none());
        assert!(args.config.is_none());
    }
    
    #[test]
    fn test_display_items() {
        let cli = Cli {
            args: Args {
                server_url: None,
                api_key: None,
                username: None,
                password: None,
                alsa_device: "default".to_string(),
                config: None,
            },
        };
        
        let items = vec![
            MediaItem {
                id: "item1".to_string(),
                name: "Test Item 1".to_string(),
                media_type: "Audio".to_string(),
                overview: Some("Test Overview 1".to_string()),
                path: Some("/path/to/item1".to_string()),
                is_folder: false,
                run_time_ticks: None,
            },
            MediaItem {
                id: "item2".to_string(),
                name: "Test Item 2".to_string(),
                media_type: "Folder".to_string(),
                overview: None,
                path: None,
                is_folder: true,
                run_time_ticks: None,
            },
        ];
        
        cli.display_items(&items);
        
    }
    
    #[test]
    fn test_display_error() {
        let cli = Cli {
            args: Args {
                server_url: None,
                api_key: None,
                username: None,
                password: None,
                alsa_device: "default".to_string(),
                config: None,
            },
        };
        
        let error = std::io::Error::new(std::io::ErrorKind::Other, "Test error");
        cli.display_error(&error);
    }
    
}
