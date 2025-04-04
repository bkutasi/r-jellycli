//! Command-line interface implementation

use clap::Parser;
use std::error::Error;
use std::io::{self, Write};

use crate::jellyfin::MediaItem;

/// Command-line arguments for r-jellycli
#[derive(Parser, Debug)]
#[command(author, version, about = "Rust Jellyfin CLI Client", long_about = None)]
pub struct Args {
    /// Jellyfin server URL
    #[arg(short, long, env = "JELLYFIN_URL")]
    pub server_url: Option<String>,

    /// Jellyfin API key
    #[arg(short, long, env = "JELLYFIN_API_KEY")]
    pub api_key: Option<String>,
    
    /// Username for Jellyfin login
    #[arg(short, long, env = "JELLYFIN_USERNAME")]
    pub username: Option<String>,
    
    /// Password for Jellyfin login
    #[arg(short, long, env = "JELLYFIN_PASSWORD")]
    pub password: Option<String>,

    /// ALSA device to use
    #[arg(short = 'd', long, default_value = "default", env = "JELLYCLI_ALSA_DEVICE")]
    pub alsa_device: String,
    
    /// Config file path
    #[arg(short, long, env = "JELLYCLI_CONFIG")]
    pub config: Option<String>,
}

/// CLI user interface for interacting with the application
pub struct Cli {
    pub args: Args,
}

impl Cli {
    /// Create a new CLI instance
    pub fn new() -> Self {
        Cli {
            args: Args::parse(),
        }
    }

    /// Display a list of media items
    pub fn display_items(&self, items: &[MediaItem]) {
        eprintln!("\nAvailable Media Items:");
        eprintln!("{:<5} {:<30} {:<15} {}", "#", "Name", "Type", "ID");
        eprintln!("{}", "-".repeat(80));
        
        for (index, item) in items.iter().enumerate() {
            let name = if item.name.len() > 28 {
                format!("{:.25}...", item.name)
            } else {
                item.name.clone()
            };
            eprintln!("{:<5} {:<30} {:<15} {}",
                index + 1,
                name,
                item.media_type,
                item.id
            );
        }
        eprintln!();
    }

    /// Get username and password interactively if needed
    pub fn get_credentials(&self) -> Result<(String, String), Box<dyn Error>> {
        let username = match &self.args.username {
            Some(u) => u.clone(),
            None => {
                print!("Enter Jellyfin username: ");
                io::stdout().flush()?;
                let mut input = String::new();
                io::stdin().read_line(&mut input)?;
                input.trim().to_string()
            }
        };
        
        let password = match &self.args.password {
            Some(p) => p.clone(),
            None => {
                print!("Enter Jellyfin password: ");
                io::stdout().flush()?;
                let mut input = String::new();
                io::stdin().read_line(&mut input)?;
                input.trim().to_string()
            }
        };
        
        Ok((username, password))
    }
    
    /// Display playback information
    pub fn display_playback_status(&self, item: &MediaItem) {
        eprintln!("\nNow playing: {}", item.name);
        eprintln!("Type: {}", item.media_type);
        
        if let Some(overview) = &item.overview {
            eprintln!("\nOverview: {}", overview);
        }
        
    }
    
    /// Display error messages
    pub fn display_error(&self, error: &dyn Error) {
        eprintln!("Error: {}", error);
    }
}
