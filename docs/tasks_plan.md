# Tasks and Project Planning

## Project Status

**Current Status**: Early Development

**Project Phase**: Core Functionality Implementation

**Last Updated**: April 4, 2025 (Playback Reporting Refactor Documentation)

## Priority Tasks

### High Priority

1. **Implement Real Streaming & Decoding**
   - Replace placeholder audio buffer with actual streaming logic. (Done)
   - Integrate a decoding library (`symphonia`). (Done)
   - Implement proper buffering and error handling. (Basic implementation done, needs refinement)
   - Implement audio resampling (`rubato`). (Done)
   - Implement correct ALSA writing with underrun handling. (Done)
   - Status: Mostly Complete (Core pipeline functional, buffering/error handling needs refinement)
   - Estimate: 1 day (Refinement)

2. **Playback Controls Enhancement**
   - Status: Partially Implemented (State reporting via HTTP POST is done; Remote `PlayNow` and `Stop` implemented; ALSA-level pause/seek/volume control is TODO)
   - Estimate: 1-2 days (Remaining for ALSA controls)

### Medium Priority

1. **Project Structure Refinement**
   - Improve error handling and return types
   - Add comprehensive documentation (ongoing)
   - Add more extensive testing
   - Status: In Progress (Significant progress made via core component refactoring, ongoing for docs/testing)
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

11. **Audio Playback Debugging & Fixes**
   - Resolved various playback issues including sample rate mismatches and ALSA underruns.
   - Implemented audio resampling using `rubato` to handle sample rate mismatches.
   - Corrected ALSA underrun (EPIPE) handling to retry writes instead of skipping data.
   - Refactored shutdown logic into `async fn shutdown` to prevent panics in `Drop`.
   - Confirmed playback pipeline (decode, resample, write) is functional.
    - Resolved application hang on shutdown (Ctrl+C) previously caused by ALSA device closing issues, as an indirect result of pause/resume logic fixes.
   - Status: Completed
9. **Playback State Reporting Refactoring & Implementation**
   - Switched reporting from WebSocket to HTTP POST.
   - Implemented a dedicated asynchronous task (`run_reporting_task`) for handling all playback reporting (Start, Progress, Stop) via HTTP POST requests using `jellyfin::reporter`.
   - Established communication between the main `Player` loop and the reporting task using an `mpsc` channel (`ReportingCommand`).
   - Implemented shared state (`SharedProgress` via `Arc<TokioMutex<...>>`) for the reporting task to access live playback position updates from the audio loop.
   - Debugged and ensured accurate reporting timing and state synchronization.
   - Status: Completed

10. **Core Component Refactoring**
   - Refactored `jellyfin::websocket`, `jellyfin::api`, and `audio::playback` modules.
   - Extracted `ws_incoming_handler.rs` from `jellyfin/websocket.rs`.
   - Decomposed `src/audio/playback.rs` into smaller modules (`alsa_writer.rs`, `processor.rs`, `state_manager.rs`, `loop_runner.rs`, etc.).
   - Improved modularity, maintainability, and logging.
   - Fixed associated build errors and runtime playback issues (e.g., premature task termination) discovered during refactoring.
   - Status: Completed

12. **Jellyfin Remote Control Command Handling & Fixes**
    *   Implemented handling for `PlayNow` command to correctly stop existing playback and start new track/queue.
    *   Implemented handling for `Stop` command to trigger graceful application shutdown.
    *   Resolved issue where subsequent `PlayNow` commands failed.
    *   Fixed application hang during shutdown (Ctrl+C or remote `Stop`).
    *   Corrected capabilities reporting to exclude non-standard commands (e.g., "Stop"), resolving HTTP 400 errors.
    *   Ensured correct ALSA device usage.
    *   Removed unnecessary volume control capabilities/handling.
    *   Status: Completed

## Backlog

*(Empty)*

## Timeline

### Short-term (1-2 weeks)
- Complete remaining high-priority tasks
- Improve audio playback quality and controls (ALSA-level pause/seek/volume)

### Medium-term (1-2 months)
- Implement medium-priority tasks
- Improve error handling and recovery

### Long-term (3+ months)
- Address backlog items
- Support additional platforms beyond Linux