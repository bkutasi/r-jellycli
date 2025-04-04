// Removed PlaybackOrchestrator import, Player handles backend internally
use tracing::{error, info, warn, instrument};
use tracing_subscriber::{fmt, EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};
use r_jellycli::config::Settings;
use r_jellycli::jellyfin::JellyfinClient;
use r_jellycli::player::{Player, PlayerCommand}; // Removed unused InternalPlayerStateUpdate
use r_jellycli::ui::Cli;
use tokio::sync::{broadcast, mpsc}; // Removed Mutex import
use r_jellycli::init_app_dirs;
use std::error::Error;
use std::path::Path;
use std::fs;
use std::sync::Arc;
// Removed unused import: use std::sync::atomic::{AtomicBool, Ordering};
// Removed log::info import, using tracing::info now
use uuid::Uuid;
use serde::{Deserialize, Serialize};
use crossterm::terminal;
use std::process;

// TerminalCleanup struct removed. Cleanup is now handled explicitly in Ctrl+C handler.

/// Creates the Player instance and its command sender channel.
#[instrument(skip_all, name = "player_setup")]
fn setup_player(settings: &Settings, jellyfin_client: JellyfinClient) -> (Player, mpsc::Sender<PlayerCommand>) {
   info!("Setting up player instance...");

   // Define channel capacities (adjust as needed)
   const STATE_UPDATE_CAPACITY: usize = 100;
   const COMMAND_BUFFER_SIZE: usize = 50;

   let (player, player_command_tx) = Player::new(
       Arc::new(jellyfin_client), // Wrap client in Arc
       settings.alsa_device.clone(),
       STATE_UPDATE_CAPACITY,
       COMMAND_BUFFER_SIZE,
   );
   info!("Created Player instance and command channel.");

   (player, player_command_tx)
}

#[tokio::main]
#[instrument(skip_all, name = "main")] // Add top-level span, skip args
async fn main() -> Result<(), Box<dyn Error>> {
    // Initialize tracing subscriber for structured JSON logging
    // Controlled by RUST_LOG env var (e.g., RUST_LOG=r_jellycli=info,warn)
    tracing_subscriber::registry()
        .with(EnvFilter::from_default_env())
        .with(fmt::layer()) // Output logs in human-readable format
        .init();

    // Create the cleanup guard. Its Drop impl runs automatically on exit.

    // Create a broadcast channel for shutdown signals for background tasks
    let (shutdown_tx, _) = broadcast::channel::<()>(1);
    // Create a flag to signal app termination (still needed for main loop and Ctrl+C handler)
    let running = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let r = running.clone();

    let shutdown_tx_clone = shutdown_tx.clone();
    tokio::spawn(async move {
        if let Ok(()) = tokio::signal::ctrl_c().await {
            r.store(false, std::sync::atomic::Ordering::SeqCst); // Use full path for Ordering

            // Explicitly disable raw mode here before signaling shutdown
            if let Err(e) = terminal::disable_raw_mode() {
                error!("Failed to disable terminal raw mode in Ctrl+C handler: {}", e);
            }
            let _ = shutdown_tx_clone.send(());
        }
    });

    let _init_span = tracing::info_span!("initialization").entered();

    let cli = Cli::new();
    let args = &cli.args;
    
    init_app_dirs()?;
    
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
        info!("Found credentials.json, attempting to load...");
        match fs::read_to_string(credentials_path) {
            Ok(creds_json) => {
                match serde_json::from_str::<Credentials>(&creds_json) {
                    Ok(creds) => {
                        info!("Loaded credentials for user: {}", creds.username);
                        // Override settings with credentials.json if command line arguments aren't provided
                        if args.server_url.is_none() {
                            settings.server_url = creds.server_url;
                        }
                        if args.username.is_none() {
                            settings.username = Some(creds.username);
                        }
                        creds_password = creds.password;
                        credentials_loaded = true;
                    },
                    Err(e) => error!("Failed to parse credentials.json: {}", e)
                }
            },
            Err(e) => error!("Failed to read credentials.json: {}", e)
        }
    }
    
    // Get password from command line, environment variable, credentials.json, or prompt
    let password = if let Some(password) = &args.password {
        password.clone()
    } else if let Ok(password) = std::env::var("JELLYFIN_PASSWORD") {
        info!("Using password from JELLYFIN_PASSWORD environment variable");
        password
    } else if credentials_loaded {
        info!("Using password from credentials.json");
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
    
    settings.validate()?;
    
    // Ensure device_id is set, generate if missing, and save
    let mut settings_updated = false;
    if settings.device_id.is_none() {
        let new_device_id = Uuid::new_v4().to_string();
        info!("Generated new Device ID: {}", new_device_id);
        settings.device_id = Some(new_device_id);
        settings_updated = true;
    }

    // Save settings immediately if device_id was generated
    if settings_updated {
        if let Err(e) = settings.save(&config_path) {
            // Log warning but continue, as device_id is generated in memory anyway
            warn!("Failed to save generated Device ID to config file: {}", e);
        } else {
            info!("Saved generated Device ID to config file.");
        }
    }
    let mut jellyfin = JellyfinClient::new(&settings.server_url);

    drop(_init_span);
    let _auth_span = tracing::info_span!("authentication").entered();

    let password_provided = args.password.is_some() || std::env::var("JELLYFIN_PASSWORD").is_ok();

    if password_provided {
        if let Some(username) = &settings.username {
            // Always authenticate with username/password if password was provided
            info!("Authenticating with username: {}", username);
            // Use the password obtained earlier (from args, env var, creds file, or prompt)
            let auth_response = jellyfin.authenticate(username, &password).await?;

            settings.user_id = Some(auth_response.user.id.clone());
            settings.api_key = Some(auth_response.access_token.clone());

            info!("Authentication successful, saving new credentials...");
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
        info!("Using existing API key for authentication.");
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
    drop(_auth_span);

    // --- Setup Player ---
    let (mut player, player_command_tx) = setup_player(&settings, jellyfin.clone());
    // Subscribe to player state updates *before* starting the player task
    let player_state_rx = player.subscribe_state_updates();
    info!("Subscribed to player state updates.");

    // --- Spawn Player Task ---
    let player_task_handle = tokio::spawn(async move {
        info!("Spawning player run loop task...");
        player.run().await;
        info!("Player run loop task finished.");
    });

    // --- Initialize Jellyfin Session and Link Player ---
    // This internally calls report_capabilities and starts WebSocket listener
    let _session_span = tracing::info_span!("session_initialization").entered();
    info!("Initializing Jellyfin session...");
    // 1. Initialize session (reports capabilities, connects WebSocket, creates handler)
    // Pass the shutdown sender clone instead of the running flag Arc
    if let Err(e) = jellyfin.initialize_session(settings.device_id.as_ref().unwrap(), shutdown_tx.clone()).await {
        warn!("Failed to initialize session (capabilities report or WebSocket connect): {}. Client may not be visible or controllable.", e);
        // Decide if this is fatal. For now, we continue but WS features might fail.
    } else {
        info!("Session initialized successfully (capabilities reported, WebSocket connected).");

        // 2. Start the WebSocket listener task, passing the necessary channels
        // Pass the command sender and state receiver to the listener
        if let Err(e) = jellyfin.start_websocket_listener(
            player_command_tx.clone(),
            player_state_rx,
            shutdown_tx.subscribe()
        ).await {
             error!("Failed to start WebSocket listener task: {}", e);
             // This is likely a significant issue, consider if the app should exit.
        } else {
             info!("WebSocket listener task started successfully.");
        }
        drop(_session_span);
    }
    
    // Check if we're in test mode (just testing authentication)
    let test_mode = std::env::var("JELLYCLI_TEST_MODE").map(|v| v == "1").unwrap_or(false);
    
    if test_mode {
        info!("Authentication successful! Test mode enabled, exiting.");
        info!("Access token: {}", settings.api_key.as_ref().unwrap_or(&"None".to_string())); // Use unwrap_or for safety
        info!("User ID: {}", settings.user_id.as_ref().unwrap_or(&"None".to_string())); // Use unwrap_or for safety
        return Ok(());
    }
    // --- Background Task Management ---
    // We don't need to manually track task handles here anymore,
    // as the Player instance manages its own playback/reporter tasks.

    // Settings are already loaded and used for initialization.

    // --- Main Application Loop ---
    let _main_loop_span = tracing::info_span!("main_loop").entered();
    info!("Fetching items from server...");
    let mut current_items = jellyfin.get_items().await?;
    let _shutdown_rx = shutdown_tx.subscribe(); // Receiver for main loop (marked unused)

    let mut current_parent_id: Option<String> = None;
    
    'main_loop: while running.load(std::sync::atomic::Ordering::SeqCst) { // Use full path for Ordering
        // Display current items

        if current_items.is_empty() {
            // Keep eprintln for direct user feedback before exit
            eprintln!("No items found in the current view. Exiting.");
            break 'main_loop;
        }

        let selected_index_option: Option<usize>; // Use Option to indicate if selection should happen

        if current_parent_id.is_none() {
            // Initial selection: Hardcode "Music" library
            info!("Attempting to select 'Music' library...");
            match current_items.iter().position(|item| item.name == "Music") {
                Some(idx) => {
                    info!("Found 'Music' library at index {}. Selecting.", idx + 1);
                    selected_index_option = Some(idx);
                }
                None => {
                    error!("'Music' library not found in the initial list!");
                    // Ensure raw mode is disabled before exiting
                    if let Err(e) = terminal::disable_raw_mode() {
                         error!("Failed to disable terminal raw mode before exit: {}", e);
                    }
                    process::exit(1);
                }
            }
        } else {
            // Subsequent selection (Artist, Album, Track): Check AUTO_SELECT_OPTION
            let auto_select_option_env = std::env::var("AUTO_SELECT_OPTION").ok();
            selected_index_option = if let Some(option_str) = auto_select_option_env {
                match option_str.parse::<usize>() {
                    Ok(idx_1_based) if idx_1_based >= 1 && idx_1_based <= current_items.len() => {
                        let idx_0_based = idx_1_based - 1;
                        info!("AUTO_SELECT_OPTION: Selecting item {} ('{}')", idx_1_based, current_items[idx_0_based].name);
                        Some(idx_0_based)
                    }
                    _ => {
                        warn!("AUTO_SELECT_OPTION ('{}') is invalid or out of bounds (1-{}). No automatic selection.",
                                 option_str, current_items.len());
                        None
                    }
                }
            } else {
                info!("AUTO_SELECT_OPTION not set. No automatic selection.");
                None
            };
        }
        
        // If Ctrl+C was pressed during selection, exit
        if !running.load(std::sync::atomic::Ordering::SeqCst) { // Use full path for Ordering
            break;
        }

        // --- Handle Selection OR Wait ---
        if let Some(selected_index) = selected_index_option {
            // --- Automatic Selection Occurred ---
            if selected_index >= current_items.len() {
                // This check should ideally be redundant if the logic above is correct, but good for safety.
                error!("Internal error: selected_index {} is out of bounds for items list (len {}). Skipping selection.", selected_index, current_items.len());
                continue;
            }

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
                continue;
            } else {
                // --- Play Selected Item ---
                // Delegate playback to the central Player instance
                info!("Selected item for playback: {} ({})", selected_item.name, selected_item.id);
                cli.display_playback_status(&selected_item); // Show what's intended to play

                let cmd = PlayerCommand::PlayNow { item_ids: vec![selected_item.id], start_index: 0 }; // Use struct variant
                if let Err(e) = player_command_tx.send(cmd).await {
                    error!("Failed to send PlayNow command to player: {}", e);
                    // Handle error appropriately, maybe break loop or show message
                } else {
                    info!("Sent PlayNow command to player task.");
                }

                // After initiating playback, wait only for the shutdown signal (Ctrl+C)
                // Keep eprintln for direct user instruction
                eprintln!("Playback started. Press Ctrl+C to exit.");
                let mut shutdown_rx_wait_playback = shutdown_tx.subscribe();
                tokio::select! {
                    _ = shutdown_rx_wait_playback.recv() => {
                         info!("Shutdown signal received during playback wait, exiting loop.");
                         break 'main_loop;
                    }
                    // We don't wait for playback completion here, only Ctrl+C.
                    // The Player manages its own lifecycle.
                }
            }
        } else {
            // --- No Automatic Selection ---
            // Wait indefinitely for WebSocket commands or Ctrl+C
            info!("No automatic selection performed. Waiting for external commands or Ctrl+C...");
            let mut shutdown_rx_wait_idle = shutdown_tx.subscribe();
            tokio::select! {
                _ = shutdown_rx_wait_idle.recv() => {
                     info!("Shutdown signal received while waiting, exiting loop.");
                     break 'main_loop;
                }
                // No other branches, just wait for shutdown
            }
        }
    }
    
    // --- Shutdown ---
    drop(_main_loop_span);
    let _shutdown_span = tracing::info_span!("shutdown").entered();
    info!("Main loop exited. Initiating graceful shutdown...");

    // --- Player Shutdown ---
    info!("Sending shutdown command to player task...");
    if let Err(e) = player_command_tx.send(PlayerCommand::Shutdown).await {
        error!("Failed to send shutdown command to player: {}", e);
    }

    info!("Waiting for player task to finish...");
    if let Err(e) = player_task_handle.await {
        error!("Error waiting for player task: {:?}", e);
    } else {
        info!("Player task finished.");
    }
    // --- WebSocket Listener Shutdown ---
    // The listener task should exit when the shutdown signal is received or its channels close.
    // Wait for the WebSocket listener task handle stored in the JellyfinClient.
    if let Some(ws_handle) = jellyfin.take_websocket_handle().await {
        info!("Waiting for WebSocket listener task to finish...");
        if let Err(e) = ws_handle.await {
            error!("Error waiting for WebSocket listener task: {:?}", e);
        } else {
            info!("WebSocket listener task finished.");
        }
    } else {
        info!("No WebSocket listener handle found to await (might not have started or already taken).");
    }

    info!("All tasks finished. Exiting application.");

    // Raw mode should have been disabled by the Ctrl+C handler or if the loop exited normally.
    // No explicit call needed here unless another exit path exists that doesn't trigger the handler.
    drop(_shutdown_span);
    Ok(())
}

