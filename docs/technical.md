# Technical Documentation

## Technology Stack

### Programming Language
- **Rust** (2021 edition): A systems programming language focused on safety, speed, and concurrency

### Core Dependencies
- **clap** (v4.1.6): Command-line argument parser with feature-rich API
- **reqwest** (v0.11): HTTP client for Rust, used for API communication
- **serde** (v1.0): Framework for serializing and deserializing Rust data structures
- **tokio** (v1.21.2): Asynchronous runtime for Rust, providing async/await functionality
- **alsa** (v0.6.0): ALSA (Advanced Linux Sound Architecture) bindings for Rust
- **serde_json** (v1.0): JSON support for serde serialization/deserialization
- **tokio-tungstenite** (v0.20.1): WebSocket client library for Tokio
- **rubato** (v0.14): High-quality audio sample rate conversion library

- **tracing** (v0.1): Framework for instrumenting Rust programs to collect structured, event-based diagnostic information.
- **tracing-subscriber** (v0.3): Utilities for implementing and composing `tracing` subscribers.
### External Dependencies
- **ALSA system libraries**: Required for audio playback on Linux systems

## Development Environment

### Build System
- **Cargo**: Rust's package manager and build system

### Environment Setup
1. Install Rust and Cargo (https://rustup.rs/)
2. Install ALSA development libraries
   ```bash
   # For Debian/Ubuntu
   apt install libasound2-dev
   
   # For Fedora/RHEL
   dnf install alsa-lib-devel
   ```
3. Clone the repository
4. Run `cargo build` to compile the project

### Building and Running
- **Development Build**: `cargo build`
- **Release Build**: `cargo build --release`
- **Run Application**:
  ```bash
  # Using command-line arguments
  cargo run -- --server-url https://your-jellyfin-server.com --username your-user --password your-pass
  
  # Or using environment variables
  JELLYFIN_SERVER_URL="https://your-jellyfin-server.com" JELLYFIN_USERNAME="your-user" JELLYFIN_PASSWORD="your-pass" cargo run
  ```

## API Integration

### Jellyfin API
- Uses Jellyfin API for authentication and media item fetching.
- Authentication supports API key, username/password via CLI args, env vars, or config file.
- Endpoints used:
  - `/Users/authenticatebyname`: For username/password authentication.
  - `/Users/{UserId}/Views`: Fetches root media views/libraries.
  - `/Users/{UserId}/Items?ParentId={ParentId}`: Fetches items within a folder.
  - `/Items/{ItemId}/Download`: Used to generate streaming URLs (indirectly).
  - `/Sessions/Playing` (POST): Reports playback start.
  - `/Sessions/Playing/Progress` (POST): Reports playback progress periodically.
  - `/Sessions/Playing/Stopped` (POST): Reports playback stop.

### API Reference
- Jellyfin OpenAPI specification is included in the project as `jellyfin-openapi-stable.json`

## Logging

The application utilizes the `tracing` crate ecosystem for structured logging, replacing the previous `log`/`env_logger` setup.

- **Framework**: `tracing` provides the core API for instrumenting code, while `tracing-subscriber` is used to configure how traces and logs are collected and output.
- **Output Streams**: 
    - Diagnostic logs (trace, debug, info, warn, error) are directed to `stdout`.
    - User-facing status messages and interactive UI elements are directed to `stderr` to keep them separate from detailed logs.
- **Log Format**: Logs sent to `stdout` use a human-readable format configured via `tracing_subscriber::fmt`. This typically includes:
    - Timestamp
    - Log level (e.g., INFO, DEBUG)
    - Active span(s)
    - Target module path (e.g., `r_jellycli::audio::playback`)
    - The log message itself.
- **Log Level Control**: Log verbosity is controlled dynamically at runtime using the `RUST_LOG` environment variable, following the standard `env_logger` directive syntax. Examples:
    - `RUST_LOG=info`: Show `info`, `warn`, and `error` messages from all crates.
    - `RUST_LOG=debug`: Show `debug` and higher messages from all crates.
    - `RUST_LOG=r_jellycli=trace`: Show `trace` and higher messages only from the `r_jellycli` crate.
    - `RUST_LOG=warn,r_jellycli::jellyfin=debug`: Show `warn` globally, but enable `debug` messages specifically for the `r_jellycli::jellyfin` module.

To enable logging, set the `RUST_LOG` environment variable before running the application:
```bash
RUST_LOG=debug cargo run -- --server-url ...
```

- Official Jellyfin API documentation: https://api.jellyfin.org/

## Code Structure

### Current Structure
- `src/main.rs`: Application entry point (binary).
- `src/lib.rs`: Library root, defining shared modules.
- `src/audio/`: Audio playback using ALSA. Refactored into multiple modules:
  - `mod.rs`: Module declaration.
  - `playback.rs`: Main `AlsaPlayer` struct and playback orchestration logic.
  - `decoder.rs`: Symphonia-based audio decoding.
  - `alsa_handler.rs`: Low-level ALSA PCM interaction.
  - `stream_wrapper.rs`: Wrapper for the HTTP stream from `reqwest`.
  - `format_converter.rs`: Sample format conversion utilities (including resampling via `rubato`).
  - `progress.rs`: Playback progress tracking structures and logic.
  - `error.rs`: Audio-specific error types.
- `src/config/`: Configuration management (`mod.rs`, `settings.rs`).
- `src/jellyfin/`: Jellyfin API client implementation:
  - `mod.rs`: Module declaration.
  - `api.rs`: Core REST API interaction logic.
  - `auth.rs`: Authentication handling.
  - `models.rs`, `models_playback.rs`: Data structures for API responses.
  - `session.rs`: Session management.
  - `websocket.rs`: WebSocket connection management and outgoing message handling.
  - `ws_incoming_handler.rs`: Logic for handling incoming WebSocket messages.
- `src/ui/`: Command-line interface and user interaction (`mod.rs`, `cli.rs`).
- `tests/`: Contains integration and manual tests.

## Data Flow

### Media Item Retrieval
1. Application authenticates (using API key or username/password).
2. Sends request to `/Users/{UserId}/Views` or `/Users/{UserId}/Items?ParentId={ParentId}`.
3. Server responds with JSON data containing media items.
4. Application parses JSON into `MediaItem` structures.
5. Media items are displayed to user for selection.

### Audio Playback
1. ALSA device is opened and configured based on settings.
2. Streaming URL is obtained via `JellyfinClient::get_stream_url`.
3. Audio data is streamed from the URL using `reqwest`.
4. Streamed data is decoded using Symphonia (`decoder.rs`).
5. If necessary, audio is resampled using `rubato` (`format_converter.rs`) to match the ALSA device's sample rate.
6. Processed audio data is written to the ALSA device (`alsa_handler.rs`).
7. Playback state (Start, Progress, Stop) is reported back to the Jellyfin server via HTTP POST requests (`api.rs`).

## Testing Strategy

### Unit Testing
- Tests for individual components (to be implemented).
- Mock objects for API and ALSA interfaces.

### Integration Testing
- End-to-end tests with mock Jellyfin server (to be implemented).

## Performance Considerations

### Audio Processing
- Buffer size and sample rate configuration for optimal playback.
- Efficient streaming to minimize memory usage.

### Network Handling
- Asynchronous API calls using tokio to prevent blocking.
- Error handling for network interruptions.

## Security Considerations

### Credential Handling
- API keys and tokens are stored in the configuration file (`~/.config/jellycli/config.json`).
- Passwords can be supplied via CLI argument or `JELLYFIN_PASSWORD` environment variable. Avoid storing passwords directly in config files or scripts.
- Consider using a secrets management tool for production environments.

### Data Protection
- No persistent storage of media implemented yet.
- HTTPS should be used for server communication.
