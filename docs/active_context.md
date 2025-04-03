# Active Development Context

**Current Focus**: Stabilizing Audio Playback and Documentation Update

**Status**:
- ✅ **Audio Playback Functional**
  - Switched playback reporting from WebSocket to HTTP POST (resolving server errors).
  - Implemented audio resampling using `rubato` to handle sample rate mismatches.
  - Corrected ALSA underrun (EPIPE) handling to retry writes instead of skipping data.
  - Confirmed playback pipeline (decode, resample, write) is functional.
  - Confirmed lack of audible sound with `hw:Loopback,0,0` is expected behavior (requires external routing), not an application bug.

- ✅ **Documentation Updates**
  - Updated README with ALSA configuration details
  - Created comprehensive `lessons-learned.mdc` document for Jellyfin integration insights
  - Documented session management requirements for remote control functionality
  - Added error handling patterns for thread-safe operation

- ✅ **User Experience Improvements**
  - Implemented clean exit handling with Ctrl+C support.
  - Refactored cleanup into `async fn shutdown` in `Player`/`PlaybackOrchestrator` to prevent panics in `Drop` related to blocking operations.
  - Application exits cleanly without panics.
   - Resolved application hang on shutdown (Ctrl+C) previously caused by ALSA device closing issues, as an indirect result of pause/resume logic fixes.

- ✅ **Jellyfin Remote Control Integration & Stable WebSocket**
  - Implemented proper client capability reporting format.
  - Established a stable and persistent WebSocket connection immediately after capabilities reporting, using the correct session ID.
  - Session keep-alive pings (every 30 seconds) successfully maintain the connection and session visibility.
  - Resolved previous error handling and thread safety issues related to the WebSocket connection.
  - Ensured `DeviceId` is correctly generated/loaded and used for session reporting.
  - The client now reliably appears in Jellyfin's "Play On" menu thanks to the stable connection.

- ✅ **SSDP Discovery Implementation**
  - Added an asynchronous SSDP broadcasting task (`src/ssdp/broadcaster.rs`).
  - Integrated the task into the main application flow (`src/main.rs`).
  - Added logging to the SSDP task for monitoring.
  - Corrected compilation errors related to SSDP integration and `DeviceId` handling.

- ❌ **Failed Simplification Attempt**: Removed SSDP broadcaster, HTTP pings, and refactored session/ID handling to align with `jellycli-repo`. This did *not* resolve the client visibility issue.

- ✅ **Blockers Resolved**
  - The previous blocker regarding client visibility in the "Play On" menu is resolved due to the stable WebSocket connection.

- ✅ **Jellyfin Remote Control State Reporting (via HTTP POST)**
  - Implemented state reporting (`PlaybackStart`, `PlaybackStopped`, `ReportPlaybackProgress`) from `Player` directly to Jellyfin Server via HTTP POST requests.
  - Used shared state (`Arc<StdMutex<PlaybackProgressInfo>>`) for `AlsaPlayer` to report playback time to `Player`'s reporting task.
  - Refactored `Player` to manage background tasks for playback and HTTP reporting.
  - Reporting mechanism is functional.


- ✅ **Core Component Refactoring**
  - Refactored `src/jellyfin/websocket.rs`: Extracted incoming message handling to `src/jellyfin/ws_incoming_handler.rs` for better separation of concerns.
  - Refactored `src/jellyfin/api.rs`: Improved internal structure with helper functions for modularity and logging.
  - Refactored `src/audio/playback.rs`: Decomposed into smaller, focused modules within `src/audio/` (`decoder.rs`, `alsa_handler.rs`, `stream_wrapper.rs`, `format_converter.rs`, `progress.rs`, `error.rs`) to enhance maintainability and clarity. The main `playback.rs` now orchestrates these components.
  - **Goal**: Improve modularity, maintainability, testability, and logging across core components.
  - **Outcome**: Cleaner code structure, better separation of responsibilities, new files created as listed above.
- ✅ **`rubato` Resampler Fix**: Resolved build errors (E0599) in `src/audio/playback.rs` related to incorrect flushing (`process_last`) and method calls (`process`) on the `rubato` resampler. Corrected flushing method, ensured `Resampler` trait was in scope, and handled `MutexGuard` dereferencing properly.

**Current State Summary**:
- Audio playback pipeline (decoding, resampling, ALSA writing) is functional.
- Playback state reporting via HTTP POST is functional.
- Application exits cleanly.
- Lack of sound on `hw:Loopback,0,0` is confirmed as expected external configuration need.

**Next Steps**:
1.  **Implement Remaining Playback Controls**: Add ALSA-level implementation for pause, seek, and volume control, and integrate them with the `Player` and HTTP/WebSocket communication as needed.
2.  **Refine Buffering/Error Handling**: Improve robustness of the audio streaming and playback pipeline.
3.  **Address Other Tasks**: Continue with tasks as per `docs/tasks_plan.md`.
4.  **Update Documentation**: Ensure all documentation reflects the current state (This task).
