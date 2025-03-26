use r_jellycli::audio::AlsaPlayer;
use r_jellycli::config::Settings;
use r_jellycli::jellyfin::JellyfinClient;
use r_jellycli::ui::Cli;
use r_jellycli::init_app_dirs;
use std::error::Error;
use std::path::Path;
use std::fs;
use serde::{Deserialize, Serialize};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    // Parse command-line arguments and initialize CLI
    let cli = Cli::new();
    let args = &cli.args;
    
    // Initialize application directories
    init_app_dirs()?;
    
    // Load configuration from file or create default
    let config_path = match &args.config {
        Some(path) => Path::new(path).to_path_buf(),
        None => Settings::default_path(),
    };
    
    let mut settings = Settings::load(&config_path)?;
    
    // Override settings with environment variables or command-line arguments
    settings.server_url = args.server_url.clone()
        .or_else(|| std::env::var("JELLYFIN_SERVER_URL").ok())
        .unwrap_or(settings.server_url);
    
    settings.api_key = args.api_key.clone()
        .or_else(|| std::env::var("JELLYFIN_API_KEY").ok())
        .or(settings.api_key);
    
    settings.username = args.username.clone()
        .or_else(|| std::env::var("JELLYFIN_USERNAME").ok())
        .or(settings.username);
    
    // Credentials structure matching test_utils::Credentials
    #[derive(Deserialize, Serialize, Debug, Clone)]
    struct Credentials {
        username: String,
        password: String,
        server_url: String,
    }
    
    // Try to load credentials from credentials.json if it exists
    let mut credentials_loaded = false;
    let mut creds_password = String::new();
    let credentials_path = Path::new("credentials.json");
    
    if credentials_path.exists() {
        println!("Found credentials.json, attempting to load...");
        match fs::read_to_string(credentials_path) {
            Ok(creds_json) => {
                match serde_json::from_str::<Credentials>(&creds_json) {
                    Ok(creds) => {
                        println!("Loaded credentials for user: {}", creds.username);
                        // Override settings with credentials.json if command line arguments aren't provided
                        if args.server_url.is_none() {
                            settings.server_url = creds.server_url;
                        }
                        if args.username.is_none() {
                            settings.username = Some(creds.username);
                        }
                        // Store password for later use
                        creds_password = creds.password;
                        credentials_loaded = true;
                    },
                    Err(e) => println!("Failed to parse credentials.json: {}", e)
                }
            },
            Err(e) => println!("Failed to read credentials.json: {}", e)
        }
    }
    
    // Get password from command line, environment variable, credentials.json, or prompt
    let password = if let Some(password) = &args.password {
        password.clone()
    } else if let Ok(password) = std::env::var("JELLYFIN_PASSWORD") {
        println!("Using password from JELLYFIN_PASSWORD environment variable");
        password
    } else if credentials_loaded {
        println!("Using password from credentials.json");
        creds_password
    } else {
        // If no password provided and username is set, prompt for password
        if settings.username.is_some() && settings.api_key.is_none() {
            let (_, password) = cli.get_credentials()?;
            password
        } else {
            String::new() // Empty password if using API key auth
        }
    };
    
    // Determine ALSA device: CLI arg (if not default) > Env Var > Default
    let alsa_device_cli = args.alsa_device.clone();
    let alsa_device_env = std::env::var("ALSA_DEVICE").ok();
    
    settings.alsa_device = if alsa_device_cli != "default" {
        // Use CLI arg if it's not the default value
        alsa_device_cli
    } else if let Some(env_device) = alsa_device_env {
        // Otherwise, use env var if set
        env_device
    } else {
        // Fallback to the default value
        "default".to_string()
    };
    
    // Validate settings
    settings.validate()?;
    
    // Initialize Jellyfin client
    let mut jellyfin = JellyfinClient::new(&settings.server_url);
    
    // Authenticate with Jellyfin server
    let password_provided = args.password.is_some() || std::env::var("JELLYFIN_PASSWORD").is_ok();

    if password_provided {
        if let Some(username) = &settings.username {
            // Always authenticate with username/password if password was provided
            println!("Authenticating with username: {}", username);
            // Use the password obtained earlier (from args, env var, creds file, or prompt)
            let auth_response = jellyfin.authenticate(username, &password).await?;

            // Save user ID and token to settings
            settings.user_id = Some(auth_response.user.id.clone());
            settings.api_key = Some(auth_response.access_token.clone());

            // Save updated settings to config file
            println!("Authentication successful, saving new credentials..."); // Added log
            settings.save(&config_path)?;
            // Update the client instance with the new token/user ID immediately
            jellyfin = jellyfin.with_api_key(settings.api_key.as_ref().unwrap());
            jellyfin = jellyfin.with_user_id(settings.user_id.as_ref().unwrap());
        } else {
             // Password provided but no username. This shouldn't happen if validation passed.
             return Err("Password provided but no username specified or found.".into());
        }
    } else if let Some(api_key) = &settings.api_key {
        // Use existing API key only if no password was provided
        println!("Using existing API key for authentication.");
        jellyfin = jellyfin.with_api_key(api_key);

        if let Some(user_id) = &settings.user_id {
            jellyfin = jellyfin.with_user_id(user_id);
        } else {
             // API key exists but no user ID - inconsistent state
             return Err("API key found in settings, but User ID is missing. Please re-authenticate.".into());
        }
    } else if settings.username.is_some() {
         // Username exists, but no password provided and no API key found.
         // This implies the password prompt should have run, or authentication cannot proceed.
         return Err("Username specified, but no password provided and no API key found in settings.".into());
    } else {
        // No username, no password, no API key - cannot authenticate.
        return Err("Cannot authenticate: No username, password, or API key provided or found.".into());
    }
    
    // Check if we're in test mode (just testing authentication)
    let test_mode = std::env::var("JELLYCLI_TEST_MODE").map(|v| v == "1").unwrap_or(false);
    
    if test_mode {
        println!("Authentication successful! Test mode enabled, exiting.");
        println!("Access token: {}", settings.api_key.as_ref().unwrap());
        println!("User ID: {}", settings.user_id.as_ref().unwrap());
        return Ok(());
    }
    
    // Main application loop
    println!("Fetching items from server...");
    let mut current_items = jellyfin.get_items().await?;
    let mut current_parent_id: Option<String> = None;
    
    loop {
        // Display current items
        cli.display_items(&current_items);
        
        // Get user selection
        match cli.select_item(&current_items) {
            Ok(selected_item) => {
                // If selected item is a folder, browse into it
                if selected_item.is_folder {
                    current_parent_id = Some(selected_item.id.clone());
                    current_items = jellyfin.get_items_by_parent_id(&selected_item.id).await?;
                    continue;
                }
                
                // Otherwise, play the selected item
                cli.display_playback_status(selected_item);
                
                // Get streaming URL
                let stream_url = jellyfin.get_stream_url(&selected_item.id)?;
                
                // Initialize audio player
                let mut player = AlsaPlayer::new(&settings.alsa_device);
                // Initialization now happens inside stream_decode_and_play based on audio spec
                
                // Stream, decode, play audio, and show progress
                match player.stream_decode_and_play(&stream_url, selected_item.run_time_ticks).await {
                    Ok(_) => println!("Playback finished."),
                    Err(e) => cli.display_error(&e), // Display audio-specific errors
                }
            },
            Err(e) => {
                cli.display_error(&*e);
                break;
            }
        }
        
        // After playback, check if we should go back or quit
        println!("
Options: [b]ack, [q]uit");
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        
        match input.trim().to_lowercase().as_str() {
            "b" | "back" => {
                if let Some(parent_id) = &current_parent_id {
                    // Go back to parent folder
                    current_items = jellyfin.get_items_by_parent_id(parent_id).await?;
                } else {
                    // Go back to root
                    current_items = jellyfin.get_items().await?;
                }
            },
            "q" | "quit" => break,
            _ => {
                // Do nothing, continue loop
            }
        }
    }
    
    Ok(())
}
