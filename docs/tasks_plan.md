# Tasks and Project Planning

## Project Status

**Current Status**: Early Development

**Project Phase**: Core Functionality Implementation

**Last Updated**: April 1, 2025 (State Reporting Update)

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

2. **Playback Controls Enhancement**
   - Status: Partially Implemented (State reporting via WebSocket is done; ALSA-level pause/seek/volume control is TODO)
   - Estimate: 1-2 days (Remaining for ALSA controls)

### Medium Priority

1. **Project Structure Refinement**
   - Improve error handling and return types
   - Add comprehensive documentation (ongoing)
   - Add more extensive testing
   - Status: In Progress
   - Estimate: 1-2 days

2. **Error Recovery**
   - Improve network error handling
   - Add reconnection logic for dropped connections
   - Implement graceful degradation
   - Status: Not Started
   - Estimate: 2 days

### Low Priority

1. **Cross-Platform Support**
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

8. **WebSocket Connection Stabilization**
   - Debugged and fixed persistent connection issues.
   - Established stable WebSocket communication for session reporting and remote control.
   - Status: Completed

9. **Jellyfin Remote Control State Reporting**
   - Implemented mechanism for `Player` to report state (Start, Stop, Progress) via channel to `WebSocketHandler`.
   - `WebSocketHandler` now sends `PlaybackStart`, `PlaybackStopped`, `ReportPlaybackProgress` messages to Jellyfin server.
   - Added shared state for `AlsaPlayer` to report playback time.
   - Refactored `Player` to manage background tasks for playback and reporting.
   - Status: Completed (Requires testing for UI visibility/control)

## Backlog

1. **Working remote control from other clients**

## Timeline

### Short-term (1-2 weeks)
- Complete remaining high-priority tasks
- Improve audio playback quality and controls

### Medium-term (1-2 months)
- Implement medium-priority tasks
- Improve error handling and recovery

### Long-term (3+ months)
- Address backlog items
- Support additional platforms beyond Linux