use r_jellycli::audio::AlsaPlayer;
use env_logger; // Added import
use r_jellycli::config::Settings;
use r_jellycli::jellyfin::JellyfinClient;
use r_jellycli::player::Player;
use r_jellycli::ui::Cli;
use tokio::sync::broadcast;
use r_jellycli::init_app_dirs;
use tokio::io::{stdin, AsyncBufReadExt, BufReader};
use std::error::Error;
use std::path::Path;
use std::fs;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::Mutex;
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
            // Removed the sleep and forced exit.
            // Rely on the 'running' flag and shutdown signal for graceful termination
            // in the main loop and background tasks.
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
    // This now internally calls report_capabilities and connects the WebSocket
    println!("Initializing Jellyfin session...");
    match jellyfin.initialize_session(settings.device_id.as_ref().unwrap(), running.clone()).await {
        Ok(()) => {
            println!("Session initialized successfully. Client should now be visible and controllable via WebSocket.");
            
            // WebSocket connection and listener are started within initialize_session.
            // We still need to set the player instance if the WebSocket connected successfully.
            
            // Create a Player instance that will be controlled by WebSocket messages
            let player = Arc::new(Mutex::new(Player::new()));
            
            // Set the player instance in the WebSocket handler (check if handler exists)
            // Use the mutable jellyfin instance here
            if let Err(e) = jellyfin.set_player(player.clone()).await {
                 // This might fail if WebSocket connection failed inside initialize_session
                println!("Warning: Failed to set player for WebSocket handler (WebSocket might not be connected): {}", e);
            }

            // WebSocket listener is now started internally within initialize_session.
            // No need to call start_websocket_listener separately.
            // The success/failure is indicated by the result of initialize_session.
        },
        // Error from initialize_session could be capability reporting or WebSocket connection failure
        Err(e) => println!("Warning: Failed to initialize session (capabilities or WebSocket): {}. Client may not be visible or controllable.", e),
    }
    
    // Check if we're in test mode (just testing authentication)
    let test_mode = std::env::var("JELLYCLI_TEST_MODE").map(|v| v == "1").unwrap_or(false);
    
    if test_mode {
        println!("Authentication successful! Test mode enabled, exiting.");
        println!("Access token: {}", settings.api_key.as_ref().unwrap());
        println!("User ID: {}", settings.user_id.as_ref().unwrap());
        return Ok(());
    }
    
    // --- Spawn Background Tasks ---
    let mut task_handles = Vec::new(); // Store task handles for graceful shutdown

    // Wrap final settings in Arc for sharing with background tasks
    let settings_arc = Arc::new(settings);


    // --- Main Application Loop ---
    // Main application loop
    println!("Fetching items from server...");
    let mut current_items = jellyfin.get_items().await?;
    let mut shutdown_rx = shutdown_tx.subscribe(); // Receiver for main loop

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
        let mut alsa_player = AlsaPlayer::new(&settings_arc.alsa_device);
        
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
        let jellyfin_clone = jellyfin.clone(); // Clone for the task
        let running_clone = running.clone(); // Clone for the task
        let mut progress_shutdown_rx = shutdown_tx.subscribe(); // Receiver for progress task
        let progress_handle = tokio::spawn(async move { // Spawn and get handle
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(10));
            let mut current_position: i64 = 0;
            let max_position = duration_ticks;
            
            loop {
                tokio::select! {
                    biased; // Prioritize shutdown check

                    // Branch 1: Shutdown signal received
                    _ = progress_shutdown_rx.recv() => {
                        println!("[PROGRESS-TASK] Shutdown signal received, exiting.");
                        break;
                    }

                    // Branch 2: Interval tick elapsed
                    _ = interval.tick() => {
                        // Check running flag as a secondary measure (optional, select should handle it)
                        if !running_clone.load(Ordering::SeqCst) {
                             println!("[PROGRESS-TASK] Running flag is false, exiting.");
                             break;
                        }

                        // Update progress (simulate progress by incrementing position)
                        current_position += 10_000_000; // Add 1 second (in ticks)
                        if max_position > 0 && current_position > max_position {
                            println!("[PROGRESS-TASK] Reached end of track duration, exiting.");
                            break;
                        }

                        // Report progress
                        if let Err(e) = jellyfin_clone.report_playback_progress(
                            &item_id,
                            current_position,
                            true, // is_playing
                            false // is_paused
                        ).await {
                            // Don't exit the loop on reporting errors, just log them
                            println!("Warning: Failed to report playback progress: {}", e);
                        } else {
                             println!("[CLIENT-DEBUG] Reporting playback progress..."); // Log successful report
                        }
                    }
                }
            }
            println!("[PROGRESS-TASK] Exited loop."); // Log when the loop finishes
        });
        task_handles.push(progress_handle); // Store the handle
        
        // Set up playback in a separate task that can be interrupted
        let running_clone = running.clone();
        let playback_result = tokio::select! {
            // Branch 1: Playback completes or errors
            // Pass a new receiver to the playback function
            result = alsa_player.stream_decode_and_play(
                &stream_url,
                current_items[selected_index].run_time_ticks,
                shutdown_tx.subscribe()
            ) => {
                 result
            },
            // Branch 2: Shutdown signal received via broadcast channel
            _ = shutdown_rx.recv() => {
                // Exit due to Ctrl+C or other shutdown signal
                Err("Playback interrupted by shutdown signal".into())
            }
        };
        
        // Progress task stop is handled by the select! inside it and awaiting the handle later.
        
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
    println!("Main loop exited. Waiting for background tasks to finish...");

    // Wait for the progress reporter tasks to complete
    for handle in task_handles {
        if let Err(e) = handle.await {
            println!("Error waiting for progress task: {:?}", e);
        }
    }
    println!("Progress tasks finished.");

    // Wait for the WebSocket listener task to complete
    if let Some(ws_handle) = jellyfin.take_websocket_handle().await { // Added .await
        println!("Waiting for WebSocket listener task to finish...");
        if let Err(e) = ws_handle.await {
            println!("Error waiting for WebSocket listener task: {:?}", e);
        } else {
            println!("WebSocket listener task finished successfully.");
        }
    } else {
        println!("No WebSocket listener handle found to await.");
    }

    println!("All tasks finished. Exiting application.");
    Ok(())
}

