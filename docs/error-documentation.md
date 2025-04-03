# Error Documentation and Debugging Guide

## Common Error Patterns and Solutions

### Jellyfin Remote Control and "Play On" Menu Visibility Issues

#### HTTP 400 Bad Request - Capabilities Reporting 

**Error**: Server returns 400 Bad Request when reporting capabilities

**Symptom**:
```
[SESSION] Server response status: 400 Bad Request
```

**Causes**:
1. Incorrect JSON structure - Jellyfin expects capabilities to be wrapped in a "capabilities" field
2. Incorrect command names in SupportedCommands list
3. Missing required fields in the capabilities object

4. Including non-standard commands (e.g., "Stop") or commands the client doesn't actually handle (e.g., volume controls like "SetVolume") in the `SupportedCommands` list.
**Solution**:
1. Wrap all capabilities in a "capabilities" key:
```json
{
  "capabilities": {
    "ApplicationVersion": "0.1.0",
    "Client": "r-jellycli",
    "DeviceId": "jellycli-HOSTNAME-uuid",
    "DeviceName": "HOSTNAME",
    "PlayableMediaTypes": ["Audio"],
    "QueueableMediaTypes": ["Audio"],
    "SupportedCommands": [...],
    "SupportsMediaControl": true,
    "SupportsPersistentIdentifier": false
  }
}
```

2. Use exact command names from Jellyfin API:
   - Use "SetRepeatMode" instead of "SetRepeat"
   - Use "SetShuffleQueue" instead of "SetShuffle"

#### Client Not Appearing in "Play On" Menu

**Error**: Client doesn't appear in Jellyfin's "Play On" menu despite successful session creation

**Symptoms**:
- No errors in client logs
- 204 No Content response from capabilities reporting
- No visible client in Jellyfin web interface "Play On" menu

**Possible Causes**:
1. WebSocket connection not established with session ID
2. Missing or incorrect session ID format
3. No keep-alive pings to maintain session

4.  **Failed Simplification**: An attempt to remove SSDP and align session handling with `jellycli-repo` did *not* resolve the issue, indicating the root cause is different or SSDP is required.
**Solutions (Implemented but still investigating)**:
1. Establish WebSocket connection *immediately* after reporting capabilities
2. Format session ID as `{client-name}_{device-name}_{uuid}`
3. Include sessionId in WebSocket URL: `socket?api_key={api_key}&deviceId={deviceId}&sessionId={sessionId}`
4. Implement keep-alive pings every 30 seconds

**Additional Debugging Steps**:
1. Check Jellyfin server logs for clues
2. Compare network traffic with jellycli (Go) implementation
3. Test with different Jellyfin server versions

#### Server-Side `ArgumentOutOfRangeException` (WebSocket Reporting)

**Error**: Jellyfin server logs show `System.ArgumentOutOfRangeException` when receiving WebSocket messages like `PlaybackProgress`.

**Symptom**: Errors appear in Jellyfin server logs, potentially disrupting playback state tracking.

**Cause**: Suspected issue within the Jellyfin server's handling of specific WebSocket message formats or timing, particularly `ReportPlaybackProgress`. The exact root cause within the server was difficult to pinpoint.

**Solution**: Switched playback state reporting (`PlaybackStart`, `PlaybackProgress`, `PlaybackStop`) from WebSocket messages to direct HTTP POST requests to the corresponding REST API endpoints. This completely bypassed the problematic WebSocket interaction for reporting.


### Audio Playback Issues

#### Audio Distortion/Speed Issues with Resampling (WebSocket Playback)

**Symptom:** Audio plays too fast or sounds distorted during remote playback initiated via WebSocket, specifically when the source audio sample rate (e.g., 44.1kHz) differs from the output device's sample rate (e.g., 48kHz), requiring resampling.

**Root Cause:** The `rubato` resampling library expects audio data in fixed-size chunks. The playback logic in `src/audio/playback.rs` (specifically the `_process_buffer` function or related logic handling WebSocket data) was feeding variable-sized chunks received over the network directly to the resampler.

**Resolution:** The audio processing pipeline in `src/audio/playback.rs` was modified. Before passing data to the `rubato` resampler, the incoming (potentially variable-sized) audio chunks are now buffered and processed into fixed-size chunks that match the resampler's requirements.

**Status:** Resolved.


### ALSA Errors

#### ALSA Underrun (EPIPE - Broken Pipe) During Playback

**Error**: `alsa::pcm::IO::writei` returns an error with `errno` code `EPIPE` (32).

**Symptom**: Audio playback might stutter, skip, or stop. Logs may show "ALSA write error: Broken pipe".

**Cause**: The ALSA buffer underran, meaning the application didn't supply audio data fast enough to the sound card. This is often a recoverable condition.

**Solution**:
1.  **Detect**: Check if the returned `alsa::Error` has an `errno()` equal to `libc::EPIPE`.
2.  **Recover**: Call `pcm.recover(libc::EPIPE, true)` on the ALSA PCM handle. The `true` argument indicates that the error message should be silenced.
3.  **Retry**: **Crucially**, retry writing the *same* audio chunk that failed. Do not discard the chunk.
```rust
// Inside the write loop in src/audio/playback.rs (_write_to_alsa)
match self.pcm.writei(buf) {
    Ok(frames_written) => { /* ... success ... */ }
    Err(e) => {
        if let Some(errno) = e.errno() {
            if errno == libc::EPIPE { // Underrun
                warn!("ALSA underrun occurred (EPIPE), attempting recovery...");
                match self.pcm.recover(errno, true) {
                    Ok(_) => {
                        warn!("ALSA recovery successful, retrying write.");
                        // Retry the write for the *same buffer*
                        // (Logic might involve looping or setting a flag to retry)
                        continue; // Or adjust loop logic to retry
                    }
                    Err(recover_err) => {
                        error!("ALSA recovery failed: {}", recover_err);
                        return Err(AudioError::AlsaError(recover_err));
                    }
                }
            }
        }
        // Handle other non-recoverable errors
        error!("Unhandled ALSA write error: {}", e);
        return Err(AudioError::AlsaError(e));
    }
}
```

### Rust-Specific Errors

#### Thread Safety in Error Handling

**Error**: 
```
error[E0277]: `dyn StdError` cannot be sent between threads safely
```

**Cause**: Box<dyn StdError> does not implement Send + Sync, which is required for thread-safe error handling

**Solution**: Convert errors to std::io::Error with formatted messages:
```rust
Err(JellyfinError::Other(Box::new(std::io::Error::new(
    std::io::ErrorKind::Other,
    format!("WebSocket connection error: {}", e)
))))
```

#### Private Method Access Error

**Error**:
```
error[E0624]: method `start_keep_alive_pings` is private
```

**Cause**: Attempting to call a private method from outside its module

**Solution**: Make the method public:
```rust
pub fn start_keep_alive_pings(&self) {
    // Method implementation
}
```

#### Unused Variable Warnings

**Warning**:
```
warning: unused variable: `hostname`
```

**Cause**: Variables declared but not used in the code

**Solution**: Prefix unused variables with underscore:
```rust
let _hostname = hostname::get()
```

#### Panics During Shutdown (Blocking Operations in `Drop`)

**Error**: Application panics during shutdown, often with messages related to blocking operations or runtime shutdown.
```
thread 'main' panicked at 'Cannot drop a runtime in a context where blocking is not allowed. This happens when a runtime is dropped from within an asynchronous context.'
// Or panics related to Mutex::blocking_lock
```

**Cause**: Performing potentially blocking operations (like joining tasks, acquiring `blocking_lock` on mutexes, complex I/O such as closing ALSA devices) inside the `Drop` implementation of a struct within an async context. Additionally, failing to properly await the completion of all spawned asynchronous tasks (e.g., playback, WebSocket listener, state reporter) or lacking proper synchronization and timeouts during resource cleanup (especially ALSA device closing) can lead to deadlocks or hangs during shutdown (triggered by Ctrl+C or remote `Stop` command). The `Drop` trait is synchronous and cannot safely execute blocking code or guarantee task completion when the async runtime is shutting down.

**Solution**:
1.  Avoid complex or blocking logic in `Drop` implementations for types used in async contexts.
2.  Implement an explicit `async fn shutdown(&mut self)` method on relevant structs (e.g., `Player`, `PlaybackOrchestrator`, `WebSocketHandler`).
3.  Within this `async shutdown` method, ensure **all** spawned tasks are gracefully signaled to stop and then awaited (e.g., using `tokio::task::JoinHandle::await`). Implement timeouts for potentially blocking operations like ALSA device closing (`alsa::PCM::close`) if necessary.
4.  Perform all resource cleanup (closing handles, releasing locks) within the `shutdown` method *after* tasks have completed.
5.  Call this `shutdown()` method explicitly during the application's termination sequence (e.g., in the main function after receiving a shutdown signal like Ctrl+C or a remote `Stop` command) *before* letting objects go out of scope.
```rust
// Example in main.rs
let mut player = Player::new(...);
// ... application logic ...

// Before exiting:
info!("Shutting down player...");
player.shutdown().await;
info!("Player shut down complete.");
// Now `player` can be dropped safely

**Status**: Resolved. Implementing the `async fn shutdown` pattern and ensuring all cleanup happens there has fixed shutdown panics and hangs, including the specific issue with ALSA device closing during Ctrl+C (which was also influenced by fixes in pause/resume logic affecting ALSA state).
```



#### `rubato` Resampler Build Errors (E0599)

**Error 1**: `error[E0599]: no method named 'process_last' found for mutable reference '&mut SincFixedIn<f32>'`

**Symptom**: Build fails with the above error when attempting to flush the `rubato` resampler.

**Cause**: The `process_last` method does not exist or is not the correct way to flush the specific resampler type being used.

**Solution**: Flush the resampler by calling `process` with an empty input slice and `None` for the output buffer size hint:
```rust
// Correct way to flush
let _ = resampler_instance.process(&[vec![]], None)?;
```

**Error 2**: `error[E0599]: no method named 'process' found for mutable reference '&mut MutexGuard<'_, Box<dyn Resampler<f32>>>'` (or similar type)

**Symptom**: Build fails with the above error when attempting to call the `process` method on the resampler, often after fixing Error 1.

**Causes**:
1.  **Trait Not in Scope**: The `rubato::Resampler` trait, which defines the `process` method, is not imported into the current scope.
2.  **Calling on `MutexGuard`**: The `process` method is being called directly on the `MutexGuard` obtained from locking a `Mutex` containing the resampler, instead of on the resampler itself.

**Solutions**:
1.  **Import Trait**: Ensure the `Resampler` trait is imported:
    ```rust
    use rubato::Resampler;
    ```
2.  **Dereference `MutexGuard`**: Dereference the `MutexGuard` to access the underlying resampler object before calling the method:
    ```rust
    // Assuming `resampler_mutex` is an Arc<Mutex<Box<dyn Resampler<f32>>>>
    let mut resampler_guard = resampler_mutex.lock().unwrap();
    // Correct: Dereference the guard
    let output_frames = (&mut *resampler_guard).process(&input_frames, None)?;
    ```

**Note**: These errors often occur together when initially integrating or modifying `rubato` usage, especially involving mutexes for thread safety.

## Recovery Strategies

### Session Recovery

If the session becomes invalid or disappears from the "Play On" menu:

1. Implement reconnection logic that re-reports capabilities
2. Establish a new WebSocket connection with the new session ID
3. Resume keep-alive pings with the new session

### Clean Error Handling

For robust error handling:

1. Log detailed error information with context
2. Use proper error types that implement Send + Sync for thread safety
3. Implement graceful recovery for network issues
4. Add retry logic with exponential backoff for transient failures
