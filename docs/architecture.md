# Architecture Document

## System Overview

The r-jellycli application is a command-line interface client for Jellyfin media servers, written in Rust. It follows a modular design approach with clear separation of concerns between components responsible for API communication, media playback, user interface, and configuration management.

## Component Architecture

### High-Level Components

```
┌───────────────────┐     ┌───────────────────┐     ┌───────────────────┐
│                   │     │                   │     │                   │
│  CLI Interface    │────▶│  Jellyfin API     │────▶│  Media Playback   │
│  (User Input)     │     │  Client           │     │  Engine           │
│                   │     │                   │     │                   │
└───────────────────┘     └───────────────────┘     └───────────────────┘
        ▲                         ▲                         ▲
        │                         │                         │
        │                         │                         │
        └─────────────────────────┼─────────────────────────┘
                                  │
                      ┌───────────────────────┐
                      │                       │
                      │  Configuration        │
                      │  Management           │
                      │                       │
                      └───────────────────────┘
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
- **Responsibility**: Communication with Jellyfin server API
- **Implementation**: `src/jellyfin/api.rs`, `src/jellyfin/auth.rs`, `src/jellyfin/models.rs`
- **Key Features**:
  - Authentication with API key or username/password
  - Support for Jellyfin API endpoints
  - Fetching media libraries and items
  - Handling parent/child navigation
  - Streaming URL generation

#### 3. Media Playback Engine (`audio` module)
- **Responsibility**: Playing audio through ALSA interface
- **Implementation**: `src/audio/playback.rs`
- **Key Features**:
  - ALSA device setup and configuration
  - Audio stream processing
  - Basic playback functionality

#### 4. Configuration Management (`config` module)
- **Responsibility**: Managing user settings and configuration
- **Implementation**: `src/config/mod.rs`, `src/config/settings.rs`
- **Key Features**:
  - Loading/saving configuration from/to file
  - Environment variable support
  - Command-line argument override

## Data Flow

1. **Configuration Loading** → Load settings from config file or create defaults
2. **User Input** → User provides server URL, credentials, and device selection via command-line or config
3. **Authentication** → Application authenticates with Jellyfin server
4. **Media Discovery** → Application retrieves and displays available media items
5. **Navigation** → User navigates through folders and selects media
6. **Streaming** → Application streams selected media from server
7. **Playback** → Audio is played through the specified ALSA device
8. **Configuration Update** → Any new settings are saved back to config file

## Key Interfaces

### External Interfaces

1. **Jellyfin REST API**
   - Used for authentication, metadata retrieval, and media streaming
   - Communication via HTTP/HTTPS using reqwest library
   - Support for both standard and Emby API endpoints

2. **ALSA Audio Interface**
   - Used for audio playback on Linux systems
   - Accessed through the alsa crate
   - Configurable device selection

### Internal Interfaces

1. **JellyfinClient**
   - Provides methods for interacting with Jellyfin server
   - Handles authentication state management
   - Methods for retrieving media items and streaming URLs

2. **AlsaPlayer**
   - Provides methods for audio playback
   - Handles device initialization and streaming
   - Currently limited to basic playback controls

3. **Settings**
   - Manages configuration state
   - Provides methods for loading/saving configuration
   - Validates configuration parameters

## Security Considerations

- API keys and tokens are stored in configuration file
- Password can be supplied via environment variable for better security
- HTTPS supported for secure communication with Jellyfin server
- User credentials are persisted only after successful authentication

## Technical Constraints

- ALSA dependency limits playback to Linux and similar systems
- Command-line interface requires terminal access
- Current implementation focuses on audio playback only
- Limited format support for audio files

## Future Architectural Enhancements

1. **Error Handling**
   - Implement more robust error handling and recovery mechanisms
   - Add better logging and debugging

2. **Testing Framework**
   - Expand unit and integration testing coverage
   - Add mocks for Jellyfin API for better testing

3. **Playback Controls**
   - Add support for pause, seek, and volume control
   - Implement better buffer management

4. **Platform Abstraction**
   - Create platform abstraction layer for audio playback
   - Enable cross-platform support beyond Linux
