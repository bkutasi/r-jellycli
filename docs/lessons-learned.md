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

        5.  **Reporting via HTTP POST (Resolution for WebSocket Errors)**:
            *   **Problem**: Persistent `System.ArgumentOutOfRangeException` errors on the Jellyfin server side when receiving WebSocket messages like `PlaybackProgress`. The exact cause within the server's WebSocket handling remained elusive.
            *   **Solution**: Switched playback reporting (`PlaybackStart`, `PlaybackProgress`, `PlaybackStop`) from WebSocket messages to direct HTTP POST requests to the corresponding REST API endpoints (`/Sessions/Playing`, `/Sessions/Playing/Progress`, `/Sessions/Playing/Stopped`).
            *   **Outcome**: This completely resolved the server-side exceptions and proved to be a more robust method for state reporting in this case.

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

## Audio Processing Lessons

        1.  **Sample Rate Mismatches (Resampling)**:
            *   **Problem**: Decoded audio stream's sample rate might not match the rate supported or configured for the ALSA output device, leading to playback speed issues or errors.
            *   **Solution**: Integrated the `rubato` crate (v0.14) for high-quality asynchronous audio resampling. The `FormatConverter` (`src/audio/format_converter.rs`) now uses `rubato` to convert the sample rate of the decoded audio chunks to match the ALSA device's target rate before writing.
            *   **Benefit**: Ensures correct playback speed regardless of source/sink rate differences.

        2.  **ALSA Underrun (EPIPE) Handling**:
            *   **Problem**: ALSA PCM writes can return an `EPIPE` (Broken pipe) error, indicating an underrun. The previous logic might incorrectly skip audio chunks after such a recoverable error.
            *   **Solution**: Implemented proper handling in `_write_to_alsa` (`src/audio/playback.rs`). When a recoverable underrun (`EPIPE`) occurs, the code now calls `pcm.recover(err.errno(), true)` and *retries* the write operation for the *same* chunk instead of discarding it.
            *   **Benefit**: Prevents audio data loss during recoverable underruns, leading to smoother playback.


        3.  **`rubato` Resampler API Nuances**:
            *   **Problem**: Build errors (E0599) occurred when trying to flush or process audio with `rubato`, especially when used with `Mutex`.
            *   **Lessons**:
                *   **Trait Scope**: The `rubato::Resampler` trait must be explicitly imported (`use rubato::Resampler;`) for its methods (like `process`) to be available.
                *   **Flushing**: The correct way to flush remaining samples from the resampler is `resampler.process(&[vec![]], None)?`, not a non-existent `process_last` method.
                *   **MutexGuard Dereferencing**: When the `Resampler` is held within a `Mutex`, the `MutexGuard` must be dereferenced (`&mut *guard`) before calling trait methods like `process`. Calling methods directly on the guard will fail as the guard itself doesn't implement the trait.
            *   **Benefit**: Understanding these specifics prevents common build errors and ensures correct interaction with the `rubato` library, particularly in concurrent contexts.


        4.  **`rubato` Resampler Input Chunk Size Requirement**:
            *   **Problem**: Audio distortion (playing too fast) occurred during remote playback (WebSocket) when resampling was needed (e.g., 44.1kHz to 48kHz).
            *   **Cause**: The `rubato` resampler requires input audio data to be provided in fixed-size chunks. Feeding it variable-sized chunks (as received over the network) directly caused processing errors leading to speed distortion.
            *   **Lesson**: When using `rubato`, ensure that incoming audio data (especially from variable sources like network streams) is buffered and processed into fixed-size chunks *before* being passed to the resampler. Proper buffer management upstream of the resampler is critical for correct operation.
            *   **Benefit**: Prevents resampling artifacts and ensures accurate playback speed when dealing with variable input buffer sizes.

        5.  **Interplay Between Playback State and Shutdown Stability**:
            *   **Problem**: Application would hang during shutdown (Ctrl+C) specifically when closing the ALSA device.
            *   **Observation**: This hang was resolved *indirectly* after implementing fixes and refinements to the pause/resume logic within the ALSA handler.
            *   **Lesson**: The state of the ALSA device (e.g., whether it's paused, running, drained) significantly impacts the behavior of `pcm.close()`. Issues in state management during playback (like improper handling of pause/resume) can manifest as seemingly unrelated problems during shutdown. Robust state management throughout the playback lifecycle is crucial not only for playback itself but also for ensuring clean resource release on exit.
            *   **Benefit**: Recognizing these dependencies helps in debugging shutdown issues by considering the preceding playback state logic.


### Tokio Task Management, Concurrency, and Shutdown

1.  **Managing Concurrent Playback Tasks (`PlayNow` Handling)**:
    *   **Problem**: Implementing the `PlayNow` command correctly, where a new track/queue needs to start immediately, replacing any current playback, proved challenging.
    *   **Challenge**: Ensuring the *currently running* playback task (e.g., managed by `PlaybackOrchestrator`) and its associated shared resources (especially the ALSA device handle) were fully stopped, cleaned up, and released *before* initiating the new playback task.
    *   **Lesson**: Simply spawning a new task for the new track is insufficient. State transitions require careful management. It's crucial to explicitly signal the current task to stop, `await` its completion (e.g., using its `JoinHandle`), and ensure all resources it held (like the ALSA handle) are closed/dropped *before* creating and starting the new task and allocating resources to it.
    *   **Pattern**: For `PlayNow`, the sequence should be: 1. Signal current playback task to stop. 2. Await task completion. 3. Clean up/close resources (e.g., ALSA handle). 4. Clear playback queue/state. 5. Start new playback task with the new item(s). 6. Allocate resources to the new task.

2.  **Graceful Shutdown Synchronization**: 
    *   **Problem**: Application hanging during shutdown (Ctrl+C or remote `Stop` command) despite having an `async fn shutdown` pattern.
    *   **Challenge**: Ensuring *all* spawned asynchronous tasks (playback, WebSocket listener, state reporter, etc.) terminate cleanly and that potentially blocking operations (like closing the ALSA device) complete without causing deadlocks.
    *   **Lesson**: Robust shutdown requires more than just a shutdown signal. It needs careful synchronization:
        *   Use explicit shutdown signals (e.g., broadcast channels) for all long-running tasks.
        *   In the main `shutdown` logic, explicitly `await` the `JoinHandle` of *each* spawned task to ensure they have fully completed their cleanup.
        *   Handle potentially blocking I/O operations during cleanup (like `alsa::PCM::close()`) carefully, potentially using `tokio::task::spawn_blocking` or implementing timeouts if they risk deadlocking the shutdown process.
        *   The `async fn shutdown` pattern (avoiding blocking ops in `Drop`) is necessary but must be combined with explicit task joining and careful handling of blocking resource cleanup.


3.  **Task Lifecycle Management in Modular Systems (Audio Refactoring)**:
    *   **Problem**: During the refactoring of the audio subsystem (`src/audio/`) into smaller modules (`loop_runner.rs`, `processor.rs`, `alsa_writer.rs`, `state_manager.rs`), issues arose with tasks terminating prematurely or not cleaning up resources correctly, leading to playback failures or hangs.
    *   **Challenge**: Ensuring that the interconnected tasks within the refactored audio module manage their lifecycles correctly, handle errors gracefully, and coordinate shutdown signals effectively.
    *   **Lesson**: Breaking down a complex task (like audio playback) into smaller, cooperating asynchronous tasks requires careful design of communication channels (e.g., for data flow, state updates, shutdown signals) and error propagation. Each task must handle its specific errors and signal failure appropriately to its coordinator (e.g., `loop_runner` or `audio_task_manager`). Furthermore, ensuring that resources (like ALSA handles or shared state via `state_manager`) are consistently managed and released across these tasks, even in error scenarios, is critical to prevent deadlocks or unexpected terminations.
    *   **Pattern**: Use clear ownership or shared ownership (`Arc`) for resources. Employ channels (like `tokio::sync::mpsc` or `watch`) for state and data flow. Propagate errors upwards using `Result` types. Ensure coordinator tasks `await` child tasks during shutdown or error recovery.

1. **Thread Safety Issues**
   - **Problem**: Box<dyn StdError> was not Send + Sync safe.
   - **Solution**: Properly wrapped errors in std::io::Error with string messages instead of passing raw dynamic error types.

2. **Method Visibility Issues**
   - **Problem**: Private start_keep_alive_pings method couldn't be called from outside the module.
   - **Solution**: Made the method public to allow proper session management.

3. **Unused Variable Warnings**
   - **Problem**: Various unused variables in the session implementation.
   - **Solution**: Prefixed unused variables with underscores to maintain code clarity.


        4. **Asynchronous Cleanup and `Drop` Trait Issues**:
           *   **Problem**: Performing potentially blocking operations (like `blocking_lock()` on a `Mutex` or joining threads/tasks) within a `Drop` implementation for asynchronous structures (like `PlaybackOrchestrator` or `Player`) can lead to panics, especially when the Tokio runtime is shutting down. `Drop` is synchronous and doesn't integrate well with async cleanup needs.
           *   **Solution**: Refactored cleanup logic into a dedicated `async fn shutdown(&mut self)` method within the relevant structs (`Player`, `PlaybackOrchestrator`). This method is called explicitly *before* the struct is dropped (e.g., before exiting `main`).
           *   **Benefit**: Ensures graceful and non-blocking cleanup of resources (stopping tasks, releasing locks) within the async context, preventing shutdown panics.
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
