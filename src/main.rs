use r_jellycli::audio::AlsaPlayer;
use r_jellycli::config::Settings;
use r_jellycli::jellyfin::JellyfinClient;
use r_jellycli::player::Player;
use r_jellycli::ui::Cli;
use r_jellycli::init_app_dirs;
use std::error::Error;
use std::path::Path;
use std::fs;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::Mutex;
use serde::{Deserialize, Serialize};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    // Create a flag to signal app termination
    let running = Arc::new(AtomicBool::new(true));
    let r = running.clone();

    // Set up Ctrl+C handler
    tokio::spawn(async move {
        if let Ok(()) = tokio::signal::ctrl_c().await {
            println!("\nReceived Ctrl+C, shutting down gracefully...");
            r.store(false, Ordering::SeqCst);
            
            // Give a short delay for graceful shutdown
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            
            // Force exit if graceful shutdown doesn't complete
            println!("Exiting r-jellycli...");
            std::process::exit(0);
        }
    });

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
    
    // Initialize session to make the client visible to other Jellyfin clients
    println!("Initializing Jellyfin session...");
    match jellyfin.initialize_session().await {
        Ok(()) => {
            println!("Session initialized successfully. Client should now be visible to other Jellyfin clients.");
            
            // Initialize WebSocket connection for remote control
            println!("Initializing WebSocket connection for remote control...");
            match jellyfin.connect_websocket().await {
                Ok(()) => {
                    // Create a Player instance that will be controlled by WebSocket messages
                    let player = Arc::new(Mutex::new(Player::new()));
                    
                    // Set the player instance in the WebSocket handler
                    if let Err(e) = jellyfin.set_player(player.clone()).await {
                        println!("Warning: Failed to set player for WebSocket handler: {}", e);
                    }
                    
                    // Start listening for WebSocket commands
                    if let Err(e) = jellyfin.start_websocket_listener(running.clone()) {
                        println!("Warning: Failed to start WebSocket listener: {}", e);
                    } else {
                        println!("WebSocket listener started successfully. Remote control is now available.");
                    }
                },
                Err(e) => println!("Warning: Failed to connect WebSocket: {}. Remote control will not be available.", e),
            }
        },
        Err(e) => println!("Warning: Failed to initialize session: {}. Client may not be visible to other Jellyfin clients.", e),
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
    
    println!("Press Ctrl+C at any time to exit the application");
    
    'main_loop: while running.load(Ordering::SeqCst) {
        // Display current items
        cli.display_items(&current_items);
        
        // Check if we're running in a non-interactive environment (like Docker)
        let auto_select_option = std::env::var("AUTO_SELECT_OPTION").ok();
        
        let selected_index = if let Some(option) = auto_select_option {
            match option.parse::<usize>() {
                Ok(idx) if idx >= 1 && idx <= current_items.len() => {
                    println!("Auto-selecting option {}: {}", idx, current_items[idx - 1].name);
                    idx - 1
                },
                _ => {
                    // In non-interactive mode with invalid option, try to find Music library
                    let music_index = current_items.iter().position(|item| 
                        item.name.to_lowercase().contains("music"));
                    
                    if let Some(idx) = music_index {
                        println!("AUTO_SELECT_OPTION not valid, auto-selecting Music library at position {}", idx + 1);
                        idx
                    } else {
                        println!("AUTO_SELECT_OPTION not valid and Music library not found, defaulting to first option");
                        0
                    }
                }
            }
        } else {
            // Interactive mode
            println!("\nEnter the number of the item to select (1-{}) or 'q' to quit: ", current_items.len());
            
            // Create a new cloned reference for the input task
            let running_for_input = running.clone();
            
            // Use a simple blocking read with periodic checks for exit signal
            let input_task = tokio::task::spawn_blocking(move || {
                let mut input = String::new();
                
                // Set up a shorter timeout for checking the exit signal
                let timeout = std::time::Duration::from_millis(100);
                
                loop {
                    // Check if we should exit frequently
                    if !running_for_input.load(Ordering::SeqCst) {
                        return "q".to_string(); // Return 'q' to ensure clean exit
                    }
                    
                    // Use a separate thread to check for input with timeout
                    let input_ready = std::io::stdin().read_line(&mut input);
                    
                    match input_ready {
                        Ok(n) if n > 0 => {
                            let trimmed = input.trim().to_lowercase();
                            return trimmed;
                        },
                        _ => {
                            // No input yet or error, sleep a bit and check exit signal again
                            std::thread::sleep(timeout);
                        }
                    }
                }
            });
            
            // Wait for input
            let input_result = tokio::time::timeout(
                std::time::Duration::from_secs(5),  // Longer timeout to prevent UI spamming
                input_task
            ).await;
            
            // Process the input result
            let input_text = match input_result {
                Ok(Ok(result)) => result,
                _ => {
                    // Either timeout or task error, check if we should exit
                    if !running.load(Ordering::SeqCst) {
                        break 'main_loop;
                    }
                    // Don't immediately redisplay the menu
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                    continue;
                }
            };
            
            match input_text.as_str() {
                "q" | "quit" => break,
                _ => {
                    // Parse selection
                    match input_text.parse::<usize>() {
                        Ok(idx) if idx >= 1 && idx <= current_items.len() => idx - 1, // Convert 1-based to 0-based
                        _ => {
                            println!("Invalid selection. Please enter a number between 1 and {} or 'q' to quit", current_items.len());
                            continue;
                        }
                    }
                }
            }
        };
        
        // If Ctrl+C was pressed during selection, exit
        if !running.load(Ordering::SeqCst) {
            break;
        }

        if selected_index >= current_items.len() {
            println!("Invalid selection");
            continue;
        }
        
        // If selected item is a folder, browse into it
        if current_items[selected_index].is_folder {
            current_parent_id = Some(current_items[selected_index].id.clone());
            current_items = jellyfin.get_items_by_parent_id(&current_items[selected_index].id).await?;
            continue;
        }
        
        // Otherwise, play the selected item
        cli.display_playback_status(&current_items[selected_index]);
        
        // Get streaming URL
        let stream_url = jellyfin.get_stream_url(&current_items[selected_index].id)?;
        
        // Initialize audio player
        let mut alsa_player = AlsaPlayer::new(&settings.alsa_device);
        
        // Update the player instance used by WebSocket handler with the current item
        println!("[PLAYER] Set current item: {}", &current_items[selected_index].id);
        
        // Report playback start to Jellyfin server
        if let Err(e) = jellyfin.report_playback_start(&current_items[selected_index].id).await {
            println!("Warning: Failed to report playback start: {}", e);
        }
        
        // Current position tracking for progress reporting
        let item_id = current_items[selected_index].id.clone();
        let duration_ticks = current_items[selected_index].run_time_ticks.unwrap_or(0);
        
        // Setup position tracking for progress updates
        let jellyfin_clone = jellyfin.clone();
        let running_clone = running.clone();
        let progress_task = tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(10));
            let mut current_position: i64 = 0;
            let max_position = duration_ticks;
            
            while running_clone.load(Ordering::SeqCst) {
                interval.tick().await;
                
                // Update progress (simulate progress by incrementing position)
                current_position += 10_000_000; // Add 1 second (in ticks)
                if max_position > 0 && current_position > max_position {
                    break;
                }
                
                // Report progress
                if let Err(e) = jellyfin_clone.report_playback_progress(
                    &item_id, 
                    current_position, 
                    true, // is_playing
                    false // is_paused
                ).await {
                    println!("Warning: Failed to report playback progress: {}", e);
                }
            }
        });
        
        // Set up playback in a separate task that can be interrupted
        let running_clone = running.clone();
        let playback_result = tokio::select! {
            result = alsa_player.stream_decode_and_play(&stream_url, current_items[selected_index].run_time_ticks) => {
                result
            },
            _ = async {
                while running_clone.load(Ordering::SeqCst) {
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
            } => {
                // Exit due to Ctrl+C
                Err("Playback interrupted by user exit request".into())
            }
        };
        
        // Ensure progress task is stopped
        progress_task.abort();
        
        // Report playback stopped
        if let Err(e) = jellyfin.report_playback_stopped(
            &current_items[selected_index].id, 
            current_items[selected_index].run_time_ticks.unwrap_or(0)
        ).await {
            println!("Warning: Failed to report playback stopped: {}", e);
        }
        
        match playback_result {
            Ok(_) => println!("Playback finished."),
            Err(e) => {
                println!("Playback error or interrupted: {}", e);
                if !running.load(Ordering::SeqCst) {
                    break; // Exit if Ctrl+C was pressed
                }
            }
        }
        
        // If Ctrl+C was pressed during playback, exit
        if !running.load(Ordering::SeqCst) {
            break;
        }
        
        // After playback, check if we should go back or quit
        println!("\nOptions: [b]ack, [q]uit, or Ctrl+C to exit");
        
        // Create a new cloned reference for the input task
        let running_for_input = running.clone();
        
        // Use a simple blocking read with periodic checks for exit signal
        let input_task = tokio::task::spawn_blocking(move || {
            let mut input = String::new();
            
            // Set up a shorter timeout for checking the exit signal
            let timeout = std::time::Duration::from_millis(100);
            
            loop {
                // Check if we should exit frequently
                if !running_for_input.load(Ordering::SeqCst) {
                    return "q".to_string(); // Return 'q' to ensure clean exit
                }
                
                // Use a separate thread to check for input with timeout
                let input_ready = std::io::stdin().read_line(&mut input);
                
                match input_ready {
                    Ok(n) if n > 0 => {
                        let trimmed = input.trim().to_lowercase();
                        return trimmed;
                    },
                    _ => {
                        // No input yet or error, sleep a bit and check exit signal again
                        std::thread::sleep(timeout);
                    }
                }
            }
        });
        
        // Wait for input
        let input_result = tokio::time::timeout(
            std::time::Duration::from_secs(5),  // Longer timeout to prevent UI spamming
            input_task
        ).await;
        
        // Process the input result
        let input_text = match input_result {
            Ok(Ok(result)) => result,
            _ => {
                // Either timeout or task error, check if we should exit
                if !running.load(Ordering::SeqCst) {
                    break;
                }
                // Don't immediately redisplay the menu
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                continue;
            }
        };
        
        match input_text.as_str() {
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
                if !running.load(Ordering::SeqCst) {
                    break;  // Exit if Ctrl+C was pressed
                }
                // By default, go back to the root menu
                current_items = jellyfin.get_items().await?;
                current_parent_id = None;
            }
        }
    }
    
    // Ensure clean exit
    println!("Exiting r-jellycli...");
    Ok(())
}
