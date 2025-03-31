# Tasks and Project Planning

## Project Status

**Current Status**: Early Development

**Project Phase**: Core Functionality Implementation

**Last Updated**: March 25, 2025

## Priority Tasks

### High Priority

1. **Implement Real Streaming & Decoding**
   - Replace placeholder audio buffer with actual streaming logic.
   - Integrate a decoding library (`symphonia`).
   - Implement proper buffering and error handling.
   - Status: In Progress (Compilation errors remain in `src/audio/playback.rs` related to `copy_interleaved_ref` and `errno` comparison)
   - Estimate: 3-4 days (Remaining)

1.1. **Fix Playback Compilation Errors**
    - Resolve issues with `copy_interleaved_ref` and `errno` in `src/audio/playback.rs`.
    - Status: Not Started
    - Estimate: 0.5 days

2. **Enhance Media Browsing**
   - Implement pagination for large libraries.
   - Add options for sorting/filtering items.
   - Status: Not Started
   - Estimate: 1-2 days

3. **Playback Controls Enhancement**
   - Add volume control
   - Implement seek functionality
   - Add pause/resume capabilities
   - Status: Not Started
   - Estimate: 2-3 days

### Medium Priority

1. **Project Structure Refinement**
   - Improve error handling and return types
   - Add comprehensive documentation (ongoing)
   - Add more extensive testing
   - Status: In Progress
   - Estimate: 1-2 days

2. **User Interface Enhancement**
   - Improve CLI interface with better formatting and colors
   - Add progress indicators for streaming (Partially implemented via `indicatif`, blocked by compilation errors)
   - Implement real-time playback information
   - Status: In Progress
   - Estimate: 1-2 days (Remaining for UI enhancements)

3. **Search Functionality**
   - Implement search across libraries
   - Add filtering by media type, genre, etc.
   - Status: Not Started
   - Estimate: 2 days

4. **Error Recovery**
   - Improve network error handling
   - Add reconnection logic for dropped connections
   - Implement graceful degradation
   - Status: Not Started
   - Estimate: 2 days

### Low Priority

1. **Playback Queue Management**
   - Implement queue for multiple item playback
   - Add shuffle and repeat modes
   - Status: Not Started
   - Estimate: 2 days

2. **Media Caching**
   - Implement local caching of frequently accessed media
   - Status: Not Started
   - Estimate: 3-4 days

3. **Cross-Platform Support**
   - Research alternatives to ALSA for Windows/macOS
   - Implement conditional compilation for platform-specific code
   - Status: Not Started
   - Estimate: 4-5 days

## Completed Tasks

1. **Project Initialization**
   - Set up Rust project with Cargo
   - Add initial dependencies
   - Status: Completed

2. **Basic CLI Structure**
   - Implement command-line argument parsing with clap
   - Create basic user interaction flow
   - Status: Completed

3. **Jellyfin API Client Implementation**
   - Add support for authentication with username/password
   - Implement API key authentication
   - Set up basic endpoints for library browsing
   - Fixed JSON parsing for media items
   - Status: Completed

4. **Configuration Management**
   - Add support for configuration file
   - Implement settings for server URL, API key, ALSA device
   - Support for environment variables for all CLI args
   - Corrected ALSA device precedence logic
   - Status: Completed

5. **Media Library Navigation**
   - Add support for browsing folders and collections
   - Implement parent/child navigation
   - Status: Completed

6. **Basic Audio Playback**
   - Implement ALSA integration
   - Add basic streaming from Jellyfin API (placeholder buffer)
   - Status: Completed

7. **Clean Exit Handling**
   - Implemented Ctrl+C signal handler for graceful termination
   - Added exit checks at all navigation points
   - Ensured proper cleanup of resources during shutdown
   - Updated documentation with exit functionality
   - Status: Completed

## Backlog

1. **Video Playback Support**
   - Research video playback libraries for Rust
   - Implement basic video playback capabilities
   - Status: Backlog
   - Estimate: 1-2 weeks

2. **Terminal UI (TUI)**
   - Implement TUI using a library like tui-rs
   - Create panels for media browsing, playback controls
   - Status: Backlog
   - Estimate: 1-2 weeks

3. **User Profile Management**
   - Support for multiple Jellyfin users
   - User preferences and history
   - Status: Backlog
   - Estimate: 3-4 days

4. **Playlist Management**
   - Create, edit, and delete playlists
   - Add/remove items from playlists
   - Status: Backlog
   - Estimate: 3-4 days

## Timeline

### Short-term (1-2 weeks)
- Complete remaining high-priority tasks
- Improve audio playback quality and controls
- Add basic search functionality

### Medium-term (1-2 months)
- Implement medium-priority tasks
- Enhance user interface
- Improve error handling and recovery

### Long-term (3+ months)
- Address backlog items
- Implement video playback
- Create full-featured TUI
- Support additional platforms beyond Linux

## Dependencies and Blockers

- ALSA dependency limits playback to Linux and similar systems
- May need additional codec libraries for expanded format support
- Network performance impacts streaming quality
