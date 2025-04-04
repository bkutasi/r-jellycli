# Architecture Document

## System Overview

The r-jellycli application is a command-line interface client for Jellyfin media servers, written in Rust. It follows a modular design approach with clear separation of concerns between components responsible for API communication, media playback, user interface, configuration management, and network discovery.

## Component Architecture

### High-Level Components (Mermaid Diagram)

```mermaid
graph TD
    subgraph "External Interfaces"
        direction LR
        JellyfinAPI[Jellyfin REST API]
        ALSA[ALSA Audio Interface]
        UDP[UDP Multicast (SSDP)]
    end

    subgraph "r-jellycli Application"
        direction TB
        CLI(CLI Interface) --> JellyfinClient(Jellyfin API Client)
        JellyfinClient --> Playback(Media Playback Engine)
        JellyfinClient --> Config(Configuration Management)
        CLI --> Config
        Playback --> Config
        SSDP(SSDP Broadcaster) --> Config
        MainApp(Main Application Logic) --> CLI
        MainApp --> JellyfinClient
        MainApp --> Playback
        MainApp --> SSDP
        MainApp --> Config
    end

    JellyfinClient -- HTTP/HTTPS --> JellyfinAPI
    Playback -- ALSA Lib --> ALSA
    SSDP -- UDP --> UDP

    style CLI fill:#f9f,stroke:#333,stroke-width:2px
    style JellyfinClient fill:#ccf,stroke:#333,stroke-width:2px
    style Playback fill:#cfc,stroke:#333,stroke-width:2px
    style Config fill:#ffc,stroke:#333,stroke-width:2px
    style SSDP fill:#cff,stroke:#333,stroke-width:2px
    style MainApp fill:#eee,stroke:#333,stroke-width:2px
```

### Component Details

#### 1. CLI Interface (`ui` module)
- **Responsibility**: Handles user input, command parsing, and output formatting
- **Implementation**: `src/ui/mod.rs`, `src/ui/cli.rs`
- **Key Features**:
  - Command-line argument parsing using Clap
  - User interaction for media selection
  - Display of media information and playback status

#### 2. Jellyfin API Client (`jellyfin` module)
- **Responsibility**: Communication with Jellyfin server API (REST and WebSocket)
- **Implementation**: `src/jellyfin/api.rs`, `src/jellyfin/auth.rs`, `src/jellyfin/models.rs`, `src/jellyfin/session.rs`, `src/jellyfin/websocket.rs`, `src/jellyfin/ws_incoming_handler.rs`
- **Key Features**:
  - Authentication with API key or username/password
  - Support for Jellyfin API endpoints
  - Fetching media libraries and items
  - Handling parent/child navigation
  - Streaming URL generation
  - Session management for remote control
  - WebSocket communication for real-time updates and commands (receiving commands).
  - **Note**: Playback state reporting (`PlaybackStart`, `PlaybackStopped`, `ReportPlaybackProgress`) is now handled via direct HTTP POST requests to the server, not WebSocket messages.

#### 3. Player Orchestrator & Playback Engine (`player` and `audio` modules)
- **Responsibility**: Manages the overall playback lifecycle (`player` module), coordinates audio decoding, processing, and output (`audio` module), and reports state back to the Jellyfin server.
- **Implementation**:
    - **`Player` (`src/player/mod.rs` & submodules)**: Central orchestrator. Spawns and manages background tasks for audio playback (`audio_task_manager.rs`), item fetching (`item_fetcher.rs`), command handling (`command_handler.rs`), and progress reporting (`reporter.rs` in `jellyfin` module). Communicates state changes directly to the Jellyfin server via HTTP POST requests. Uses broadcast channels for task shutdown.
    - **Audio Subsystem (`src/audio/mod.rs` & submodules)**: Handles the specifics of audio playback. Refactored from a monolithic `playback.rs` into:
        - `playback.rs`: Simplified entry point or coordinator for the audio subsystem.
        - `loop_runner.rs`: Manages the main audio processing loop task.
        - `processor.rs`: Contains the core logic for fetching decoded data, processing it (e.g., format conversion), and sending it to the writer.
        - `alsa_writer.rs`: Handles writing processed audio samples to the ALSA device via `alsa_handler.rs`.
        - `state_manager.rs`: Manages shared playback state (e.g., playing/paused, progress).
        - `decoder.rs`: Handles stream decoding using Symphonia.
        - `alsa_handler.rs`: Low-level interaction with the ALSA PCM device.
        - `format_converter.rs`: Converts audio samples if needed.
        - `sample_converter.rs`: Utility for sample format conversions.
        - `progress.rs`: Manages playback progress tracking (`PlaybackProgressInfo`), likely used by `state_manager.rs`.
        - `error.rs`: Defines audio-specific errors (`AudioError`).
- **Key Features**:
  - Asynchronous task management for playback and progress reporting.
  - Decoupled state communication using channels and shared memory (e.g., via `state_manager.rs`).
  - ALSA device setup and configuration.
  - Audio stream decoding and processing.
  - Playback lifecycle management (Start, Stop, Pause, Seek - check `player` module for current status).
  - Periodic progress reporting to Jellyfin server via HTTP POST requests.

#### 4. Configuration Management (`config` module)
- **Responsibility**: Managing user settings and application configuration
- **Implementation**: `src/config/mod.rs`, `src/config/settings.rs`
- **Key Features**:
  - Loading/saving configuration from/to file
  - Environment variable support
  - Command-line argument override
  - Provides settings (like DeviceId) to other components

#### 5. SSDP Broadcaster (`ssdp` module - *New*)
- **Responsibility**: Network discovery via SSDP protocol
- **Implementation**: `src/ssdp/broadcaster.rs` (to be created)
- **Key Features**:
  - Runs as an asynchronous Tokio task
  - Periodically broadcasts SSDP `NOTIFY` messages via UDP multicast
  - Uses `DeviceId` from Configuration Management
  - Enables discoverability for "Play On" functionality

## Data Flow

1.  **Configuration Loading** → Load settings from config file or create defaults.
2.  **Initialization** → Main application starts, initializes components (API Client, Config, etc.).
3.  **Task Spawning** → Spawn background tasks:
    *   SSDP Broadcaster (starts sending UDP `NOTIFY` messages).
    *   WebSocket Handler (if session is active).
    *   Player Task (waits for commands).
4.  **User Input** → User provides server URL, credentials, and device selection via command-line or config.
5.  **Authentication** → Application authenticates with Jellyfin server (HTTP).
6.  **Session Reporting** → Report capabilities to Jellyfin server (HTTP).
7.  **WebSocket Connection** → Establish persistent WebSocket connection with session ID.
8.  **Media Discovery** → Application retrieves and displays available media items (HTTP).
9.  **Navigation** → User navigates through folders and selects media.
10. **Playback Initiation** → (Local or Remote) Command received (CLI or WebSocket) to play media.
11. **Streaming URL** → Application gets playback info and streaming URL (HTTP).
12. **Playback & State Reporting** →
    *   `Player` starts the `AlsaPlayer` task, which orchestrates decoding (`decoder.rs`), ALSA interaction (`alsa_handler.rs`), and streaming (`stream_wrapper.rs`, HTTP) for playback.
    *   `Player` starts a progress reporting task that reads time from the shared state managed by `progress.rs`.
    *   `Player`'s background task reads time from the shared state managed by `progress.rs`.
    *   `Player` sends `PlaybackStart`, `PlaybackStopped`, `ReportPlaybackProgress` updates directly to the Jellyfin server via HTTP POST requests to `/Sessions/Playing`, `/Sessions/Playing/Stopped`, and `/Sessions/Playing/Progress` respectively.
13. **Configuration Update** → Any new settings are saved back to config file on exit.

## Key Interfaces

### External Interfaces

1.  **Jellyfin REST API**
    - Used for authentication, metadata retrieval, session reporting, and media streaming URLs.
    - Communication via HTTP/HTTPS using `reqwest`.

2.  **Jellyfin WebSocket API**
    - Used for real-time command/control and status updates.
    - Communication via WebSocket using libraries like `tokio-tungstenite`.

3.  **ALSA Audio Interface**
    - Used for audio playback on Linux systems.
    - Accessed through the `alsa` crate.
    - Configurable device selection.

4.  **UDP Multicast (SSDP - *New*)**
    - Used by the SSDP Broadcaster to send `NOTIFY` messages.
    - Standard address: `239.255.255.250:1900`.
    - Implemented using `tokio::net::UdpSocket`.

### Internal Interfaces

1.  **JellyfinClient**
    - Provides methods for interacting with Jellyfin server (REST & WebSocket).
    - Handles authentication state and session management.
    - Methods for retrieving media items, streaming URLs, sending commands.

2.  **Audio Subsystem (`src/audio/` modules)**
    - Provides an interface (likely via `src/audio/playback.rs` or a dedicated control structure) for the `Player` orchestrator to control playback (start, stop, pause, seek).
    - Internally manages ALSA interaction (`alsa_writer.rs`, `alsa_handler.rs`), audio processing (`processor.rs`), state (`state_manager.rs`), and the core loop (`loop_runner.rs`).

3.  **Settings**
    - Manages configuration state.
    - Provides methods for loading/saving configuration.
    - Validates configuration parameters.

4.  **SsdpBroadcaster Task**
    - Runs independently, requires configuration (`DeviceId`).
    - Interacts with the network via UDP.

5.  **Playback Reporting Mechanism (HTTP POST)**
    - **Purpose**: Communication channel from `Player` to the Jellyfin Server for state updates.
    - **Mechanism**: The `Player` component (specifically its progress reporting task) directly sends HTTP POST requests to the Jellyfin server endpoints (`/Sessions/Playing`, `/Sessions/Playing/Progress`, `/Sessions/Playing/Stopped`) to report playback status. This replaces the previous WebSocket-based reporting.

6.  **`PlaybackProgressInfo` Shared State (`Arc<StdMutex<...>>`)**
    - **Purpose**: Communication channel from the audio playback logic (`src/audio/progress.rs`) to `Player`'s progress reporting task.
    - **Data**: Contains current playback position (e.g., seconds).
    - **Mechanism**: The audio playback logic updates the state during playback; `Player`'s background task reads it periodically to send `Progress` updates via the MPSC channel.

## Security Considerations

- API keys and tokens are stored in configuration file (permissions should be restricted).
- Password can be supplied via environment variable for better security.
- HTTPS/WSS should always be preferred for secure communication with Jellyfin server.
- User credentials are persisted only after successful authentication if configured.
- Network interfaces for SSDP should be considered (binding to `0.0.0.0` is common but review security implications).

## Technical Constraints

- ALSA dependency limits playback primarily to Linux systems.
- Command-line interface requires terminal access.
- Current implementation focuses on audio playback.
- Network availability is required for discovery (SSDP) and operation.

## Future Architectural Enhancements

1.  **Error Handling & Resilience**
    - Implement more robust error handling, logging, and recovery mechanisms (e.g., task restarts, backoff).
2.  **Testing Framework**
    - Expand unit, integration, and potentially end-to-end testing coverage.
    - Add mocks for external interfaces (Jellyfin API, ALSA, SSDP).
3.  **Playback Controls & Features**
    - Fully implement pause, seek, volume control, queue management.
    - Improve buffering and format support.
4.  **Platform Abstraction**
    - Create platform abstraction layer for audio playback and potentially discovery.
5.  **SSDP M-SEARCH Handling** (Optional)
    - Implement listening for SSDP `M-SEARCH` requests and responding appropriately.
