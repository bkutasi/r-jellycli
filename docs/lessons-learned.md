# Lessons Learned from Jellyfin Client Implementation

## Jellyfin API Insights

### Session Registration Requirements

1. **Correct Session Registration Flow**
   - **Problem**: Client did not appear in Jellyfin's "play on" menu despite correct capabilities reporting.
   - **Cause**: The sequence of operations for session initialization was incorrect.
   - **Solution**: Implemented the proper sequence for Jellyfin remote control registration:
     1. Report capabilities to server → Get session ID
     2. Connect WebSocket with the session ID (critical)
     3. Start keep-alive pings

2. **Session ID Format**
   - **Problem**: Jellyfin requires a specific format for session IDs to properly recognize clients in the "play on" menu.
   - **Solution**: Adopted the session ID format from jellycli: `{client-name}_{device-name}_{uuid}` which is essential for remote control functionality.

3. **WebSocket Connection URL Format**
   - **Problem**: Jellyfin looks for the session ID in the WebSocket URL to associate the WebSocket connection with the session.
   - **Cause**: Our original implementation was missing the sessionId URL parameter in the WebSocket connection.
   - **Solution**: Added sessionId parameter to WebSocket URL: `socket?api_key={api_key}&deviceId={deviceId}&sessionId={sessionId}`

### Capabilities Reporting

1. **Capabilities JSON Structure**
   - **Problem**: Server rejected our capabilities with 400 Bad Request status.
   - **Cause**: Jellyfin API expects capabilities to be wrapped in a "capabilities" field.
   - **Solution**: Changed from structured `ClientCapabilitiesDto` to a simple map with all values inside a "capabilities" key.

2. **Command Naming Conventions**
   - **Problem**: Some command names in our implementation didn't match Jellyfin's expected values.
   - **Solution**: Updated command names to match Jellyfin expectations:
     - Changed `SetRepeat` to `SetRepeatMode`
     - Changed `SetShuffle` to `SetShuffleQueue`


### WebSocket State Reporting

1.  **Required Messages**:
    *   To make the client controllable and visible in the Jellyfin UI's "Now Playing" section, specific WebSocket messages must be sent *from* the client *to* the server.
    *   Key messages include `PlaybackStart`, `PlaybackStopped`, and `ReportPlaybackProgress`.

2.  **Progress Reporting**:
    *   `ReportPlaybackProgress` needs to be sent periodically (e.g., every few seconds) while playback is active.
    *   It requires the current playback position, typically in Ticks (10,000,000 ticks = 1 second). The `Player` needs a mechanism to get this information from the actual audio playback component (`AlsaPlayer`).

3.  **Decoupled Architecture Pattern**:
    *   Using asynchronous channels (like Tokio MPSC) to send state updates (`PlayerStateUpdate` enum) from the central `Player` to the `WebSocketHandler` works well.
    *   Using shared, thread-safe state (`Arc<StdMutex<...>>`) allows the low-level playback component (`AlsaPlayer`) to report its current position back to the `Player`'s progress reporting task without tight coupling.

4.  **Background Task Management**:
    *   The `Player` needs to manage background Tokio tasks for both audio playback and periodic progress reporting.
    *   Using broadcast channels for shutdown signals provides a clean way to terminate these tasks when playback stops or the application exits.

## Implementation Patterns from jellycli (Go)

1. **Device ID Generation**
   - **Adopted**: The device ID format uses a combination of client name, device name, and UUID for consistency.
   - **Implementation**: Consistent device ID across all communication with the server.

2. **WebSocket Connection Timing**
   - **Adopted**: Immediate WebSocket connection after capabilities reporting is essential for remote control registration.
   - **Implementation**: Established WebSocket connection immediately with session ID parameter.

3. **Keep-Alive Mechanism**
   - **Adopted**: Regular session pings (every 30 seconds) to maintain visibility in the "play on" menu.
   - **Implementation**: Background task for session keep-alive.

## Technical Implementation Challenges

1. **Thread Safety Issues**
   - **Problem**: Box<dyn StdError> was not Send + Sync safe.
   - **Solution**: Properly wrapped errors in std::io::Error with string messages instead of passing raw dynamic error types.

2. **Method Visibility Issues**
   - **Problem**: Private start_keep_alive_pings method couldn't be called from outside the module.
   - **Solution**: Made the method public to allow proper session management.

3. **Unused Variable Warnings**
   - **Problem**: Various unused variables in the session implementation.
   - **Solution**: Prefixed unused variables with underscores to maintain code clarity.

## Failed Simplification Attempts

1.  **Removing SSDP & Aligning with `jellycli-repo`**
    *   **Attempt**: Simplified the client by removing the SSDP broadcaster, HTTP keep-alive pings, and refactoring session/ID handling to closely mimic the `jellycli-repo` (Go reference client).
    *   **Outcome**: Failed. The client still did not appear in the Jellyfin "play on" menu.
    *   **Lesson**: Simply removing SSDP and aligning session handling with the Go client was insufficient. This suggests either SSDP *is* required (or contributes in a necessary way), or the core visibility issue lies elsewhere (e.g., subtle WebSocket behavior differences, server-side configuration/bugs, or other undiscovered factors). The simplification did not isolate the root cause.


## General Learning Points

1. **API Documentation Interpretation**
   - Different API endpoints might require different payload structures despite similar purposes.
   - Careful review of HTTP status codes (204 No Content is a positive response in this case).

2. **WebSocket Protocol for Remote Control**
   - WebSocket connections require synchronized timing with HTTP requests.
   - Session identification must be consistent across different connection types.

3. **Rust-specific Lessons**
   - Mutex wrapping is needed for shared data between threads.
   - Error types must implement Send + Sync for thread-safe error handling.
   - Tokio's spawn function requires 'static lifetime for all moved values.
