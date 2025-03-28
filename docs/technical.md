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

### API Reference
- Jellyfin OpenAPI specification is included in the project as `jellyfin-openapi-stable.json`
- Official Jellyfin API documentation: https://api.jellyfin.org/

## Code Structure

### Current Structure
- `src/main.rs`: Application entry point (binary).
- `src/lib.rs`: Library root, defining shared modules.
- `src/audio/`: Audio playback using ALSA (`mod.rs`, `playback.rs`).
- `src/config/`: Configuration management (`mod.rs`, `settings.rs`).
- `src/jellyfin/`: Jellyfin API client implementation (`mod.rs`, `api.rs`, `auth.rs`, `models.rs`).
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
4. Streamed data (placeholder logic currently) is written to the ALSA device.

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

### Data Protection
- No persistent storage of media or credentials implemented yet
- HTTPS should be used for server communication
