use r_jellycli::audio::PlaybackOrchestrator; // Renamed from AlsaPlayer
use log::error;
use env_logger;
use r_jellycli::config::Settings;
use r_jellycli::jellyfin::JellyfinClient;
use r_jellycli::player::Player;
use r_jellycli::ui::Cli;
use tokio::sync::{broadcast, Mutex}; // Added Mutex
use r_jellycli::init_app_dirs;
use tokio::io::{stdin, AsyncBufReadExt, BufReader};
use std::error::Error;
use std::path::Path;
use std::fs;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use log::info; // Added info log
use uuid::Uuid;
use serde::{Deserialize, Serialize};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    // Initialize logging based on RUST_LOG environment variable
    env_logger::init(); // Added initialization

    // Create a broadcast channel for shutdown signals for background tasks
    let (shutdown_tx, _) = broadcast::channel::<()>(1);
    // Create a flag to signal app termination
    let running = Arc::new(AtomicBool::new(true));
    let r = running.clone();

    // Set up Ctrl+C handler
    let shutdown_tx_clone = shutdown_tx.clone(); // Clone sender for Ctrl+C handler
    tokio::spawn(async move {
        if let Ok(()) = tokio::signal::ctrl_c().await {
            println!("\nReceived Ctrl+C, shutting down gracefully...");
            r.store(false, Ordering::SeqCst);
            
            let _ = shutdown_tx_clone.send(()); // Signal background tasks
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
    
    // Ensure device_id is set, generate if missing, and save
    let mut settings_updated = false;
    if settings.device_id.is_none() {
        let new_device_id = Uuid::new_v4().to_string();
        println!("Generated new Device ID: {}", new_device_id);
        settings.device_id = Some(new_device_id);
        settings_updated = true;
    }

    // Save settings immediately if device_id was generated
    if settings_updated {
        if let Err(e) = settings.save(&config_path) {
            // Log warning but continue, as device_id is generated in memory anyway
            println!("Warning: Failed to save generated Device ID to config file: {}", e);
        } else {
            println!("Saved generated Device ID to config file.");
        }
    }
// --- Create Core Components ---

// Create PlaybackOrchestrator instance (needs device name from settings)
let playback_orchestrator = Arc::new(Mutex::new(PlaybackOrchestrator::new(&settings.alsa_device)));
info!("Created PlaybackOrchestrator for device: {}", settings.alsa_device);

// Create the central Player state manager
let player = Arc::new(Mutex::new(Player::new()));
info!("Created central Player instance.");

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

    // --- Configure Player Dependencies ---
    { // Scope for player lock
        let mut player_guard = player.lock().await;
        player_guard.set_jellyfin_client(jellyfin.clone()); // Give Player access to the client
        // Use the existing method name, which now accepts Arc<Mutex<PlaybackOrchestrator>>
        player_guard.set_alsa_player(playback_orchestrator.clone()); // Give Player access to the audio player
        info!("Configured Player instance with JellyfinClient and PlaybackOrchestrator.");
    }


    // --- Initialize Jellyfin Session and Link Player ---
    // This internally calls report_capabilities and starts WebSocket listener
    info!("Initializing Jellyfin session...");
    // 1. Initialize session (reports capabilities, connects WebSocket, creates handler)
    if let Err(e) = jellyfin.initialize_session(settings.device_id.as_ref().unwrap(), running.clone()).await {
        log::warn!("Failed to initialize session (capabilities report or WebSocket connect): {}. Client may not be visible or controllable.", e);
        // Decide if this is fatal. For now, we continue but WS features might fail.
    } else {
        info!("Session initialized successfully (capabilities reported, WebSocket connected).");

        // 2. Set the Player instance on the WebSocket handler
        if let Err(e) = jellyfin.set_player(player.clone()).await {
            log::warn!("Failed to link Player to WebSocket handler (handler might be missing if WS connection failed): {}", e);
        } else {
            info!("Successfully linked Player to WebSocket handler.");

            // 3. Start the WebSocket listener task
            // Pass the broadcast receiver instead of the AtomicBool
            if let Err(e) = jellyfin.start_websocket_listener(shutdown_tx.subscribe()).await {
                 log::error!("Failed to start WebSocket listener task: {}", e);
                 // This is likely a significant issue, consider if the app should exit.
            } else {
                 info!("WebSocket listener task started successfully.");
            }
        }
    }
    
    // Check if we're in test mode (just testing authentication)
    let test_mode = std::env::var("JELLYCLI_TEST_MODE").map(|v| v == "1").unwrap_or(false);
    
    if test_mode {
        println!("Authentication successful! Test mode enabled, exiting.");
        println!("Access token: {}", settings.api_key.as_ref().unwrap());
        println!("User ID: {}", settings.user_id.as_ref().unwrap());
        return Ok(());
    }
    // --- Background Task Management ---
    // We don't need to manually track task handles here anymore,
    // as the Player instance manages its own playback/reporter tasks.
    // let mut task_handles = Vec::new();

    // Settings are already loaded and used for initialization.
    // let settings_arc = Arc::new(settings); // Not needed directly here anymore

    // --- Main Application Loop ---
    // Main application loop
    println!("Fetching items from server...");
    let mut current_items = jellyfin.get_items().await?;
    let _shutdown_rx = shutdown_tx.subscribe(); // Receiver for main loop (marked unused)

    let mut current_parent_id: Option<String> = None;
    
    println!("Press Ctrl+C at any time to exit the application");
    
#[derive(Debug)]
enum SelectionOutcome {
    Selected(usize),
    Quit,
    Continue,
    Error(String),
}


    'main_loop: while running.load(Ordering::SeqCst) {
        // Display current items
        cli.display_items(&current_items);
        
        // Check if we're running in a non-interactive environment (like Docker)
        let auto_select_option = std::env::var("AUTO_SELECT_OPTION").ok();
        
        let outcome = if let Some(option) = auto_select_option {
            match option.parse::<usize>() {
                Ok(idx) if idx >= 1 && idx <= current_items.len() => {
                    println!("Auto-selecting option {}: {}", idx, current_items[idx - 1].name);
                    SelectionOutcome::Selected(idx - 1)
                },
                _ => {
                    // In non-interactive mode with invalid option, try to find Music library
                    let music_index = current_items.iter().position(|item|
                        item.name.to_lowercase().contains("music"));
                    
                    if let Some(idx) = music_index {
                        println!("AUTO_SELECT_OPTION not valid, auto-selecting Music library at position {}", idx + 1);
                        SelectionOutcome::Selected(idx)
                    } else {
                        println!("AUTO_SELECT_OPTION not valid and Music library not found, defaulting to first option");
                        SelectionOutcome::Selected(0) // Default to first item
                    }
                }
            }
        } else {
            // Interactive mode
            println!("\nEnter the number of the item to select (1-{}) or 'q' to quit: ", current_items.len());
            
            // Asynchronous input handling
            let mut stdin_reader = BufReader::new(stdin());
            let mut input = String::new();
            let mut shutdown_rx_input = shutdown_tx.subscribe(); // Need a separate receiver for this select

            tokio::select! {
                // Wait for input from the user
                result = stdin_reader.read_line(&mut input) => {
                    match result {
                        Ok(0) => { // EOF reached (e.g., pipe closed)
                            println!("Input stream closed, exiting.");
                            SelectionOutcome::Quit
                        }
                        Ok(_) => {
                            let trimmed_input = input.trim().to_lowercase();
                            match trimmed_input.as_str() {
                                "q" | "quit" => SelectionOutcome::Quit,
                                _ => {
                                    match trimmed_input.parse::<usize>() {
                                        Ok(idx) if idx >= 1 && idx <= current_items.len() => SelectionOutcome::Selected(idx - 1), // Convert 1-based to 0-based
                                        _ => {
                                            println!("Invalid selection. Please enter a number between 1 and {} or 'q' to quit", current_items.len());
                                            SelectionOutcome::Continue // Ask for input again
                                        }
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            println!("Error reading input: {}. Exiting.", e);
                            SelectionOutcome::Error(format!("Input error: {}", e))
                        }
                    }
                },
                // Wait for the shutdown signal
                _ = shutdown_rx_input.recv() => {
                    // Shutdown signal received while waiting for input
                    println!("Shutdown signal received while waiting for selection.");
                    SelectionOutcome::Quit
                }
            }
        };

        // Handle the outcome of the selection
        let selected_index = match outcome {
            SelectionOutcome::Selected(idx) => idx,
            SelectionOutcome::Quit => break 'main_loop,
            SelectionOutcome::Continue => continue 'main_loop,
            SelectionOutcome::Error(msg) => {
                eprintln!("Error during selection: {}", msg);
                break 'main_loop;
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

        // --- Handle Selection ---
        let selected_item = current_items[selected_index].clone();

        // If selected item is a folder, browse into it
        if selected_item.is_folder {
            info!("Browsing into folder: {}", selected_item.name);
            current_parent_id = Some(selected_item.id.clone());
            // Fetch new items - Use the main jellyfin client instance
            match jellyfin.get_items_by_parent_id(&selected_item.id).await {
                Ok(items) => current_items = items,
                Err(e) => {
                    error!("Failed to fetch items for folder {}: {}", selected_item.id, e);
                    // Optionally go back or show error message
                    // For now, just continue the loop with old items
                }
            }
            continue; // Go back to display new items
        }

        // --- Play Selected Item ---
        // Delegate playback to the central Player instance
        info!("Selected item for playback: {} ({})", selected_item.name, selected_item.id);
        cli.display_playback_status(&selected_item); // Show what's intended to play

        // Lock the player and initiate playback
        // This will internally handle fetching stream URL, starting tasks, and reporting state
        let player_arc = player.clone(); // Clone Arc for the async block
        tokio::spawn(async move {
            let mut player_guard = player_arc.lock().await;
            // Clear existing queue and play the selected item immediately
            player_guard.clear_queue().await;
            player_guard.add_items(vec![selected_item]); // Add the single selected item
            player_guard.play_from_start().await; // Start playback
        });

        // After initiating playback, the main loop can continue or wait differently.
        // For a simple CLI, we might just loop back to show the main menu,
        // as playback now happens in the background managed by Player/WebSocket.
        // Or, we could enter a "now playing" state here if desired.
        // For now, let's just continue the loop to show the current directory again.
        // Fetch root items again if we were playing from root, or stay in current folder
        let _parent = current_parent_id.clone(); // Marked unused

        
        // If Ctrl+C was pressed during playback, exit
        if !running.load(Ordering::SeqCst) {
        // Explicitly check if shutdown was triggered during playback handling
        if !running.load(Ordering::SeqCst) {
            println!("Shutdown signal detected after playback handling, exiting main loop.");
            break 'main_loop;
        }

            break;
        }
        
        // After playback, check if we should go back or quit
        println!("\nOptions: [b]ack, [q]uit, or Ctrl+C to exit");
        
        // Asynchronous input handling for post-playback action
        let mut stdin_reader = BufReader::new(stdin());
        let mut input = String::new();
        let mut shutdown_rx_input = shutdown_tx.subscribe(); // Need a separate receiver for this select

        let input_text = tokio::select! {
            // Wait for input from the user
            result = stdin_reader.read_line(&mut input) => {
                match result {
                    Ok(0) => { // EOF reached
                        println!("Input stream closed, exiting.");
                        "q".to_string() // Treat EOF as quit
                    }
                    Ok(_) => {
                        input.trim().to_lowercase()
                    }
                    Err(e) => {
                        println!("Error reading input: {}. Exiting.", e);
                        "q".to_string() // Treat error as quit
                    }
                }
            },
            // Wait for the shutdown signal
            _ = shutdown_rx_input.recv() => {
                // Shutdown signal received while waiting for input
                println!("Shutdown signal received while waiting for action.");
                "q".to_string() // Treat shutdown as quit
            }
        };

        match input_text.as_str() {
            "b" | "back" => {
                if let Some(parent_id) = &current_parent_id {
                    // Go back to parent folder
                    // Need to handle potential error from get_items_by_parent_id
                    match jellyfin.get_items_by_parent_id(parent_id).await {
                        Ok(items) => current_items = items,
                        Err(e) => {
                            println!("Error fetching parent items: {}", e);
                            // Decide how to handle error, maybe go to root or retry?
                            // For now, let's go to root as a fallback
                            current_items = jellyfin.get_items().await?;
                            current_parent_id = None;
                        }
                    }
                } else {
                    // Go back to root
                    current_items = jellyfin.get_items().await?;
                    current_parent_id = None; // Ensure parent ID is cleared
                }
            },
            "q" | "quit" => break 'main_loop,
            _ => {
                // If Ctrl+C was pressed (input_text became "q"), this won't be reached.
                // If any other invalid input, go back to the root menu.
                println!("Invalid option, returning to root menu.");
                current_items = jellyfin.get_items().await?;
                current_parent_id = None;
            }
        }
    }
    
    // --- Shutdown ---
    println!("Main loop exited. Initiating graceful shutdown...");

    // --- Player Shutdown ---
    // Explicitly shut down the player first, which handles its own tasks and the audio orchestrator.
    println!("Shutting down player components...");
    { // Scope for player lock
        let mut player_guard = player.lock().await;
        player_guard.shutdown().await; // Call the new shutdown method
    }
    println!("Player components shut down.");

    // --- WebSocket Listener Shutdown ---
    // Now wait for the WebSocket listener task (if it was started)
    if let Some(ws_handle) = jellyfin.take_websocket_handle().await {
        println!("Waiting for WebSocket listener task to finish...");
        if let Err(e) = ws_handle.await {
            error!("Error waiting for WebSocket listener task: {:?}", e); // Use error log level
        } else {
            info!("WebSocket listener task finished successfully."); // Use info log level
        }
    } else {
        info!("No WebSocket listener handle found to await (might not have started)."); // Use info log level
    }

    println!("All tasks finished. Exiting application.");
    Ok(())
}

