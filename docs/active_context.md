# Active Development Context

**Current Focus**: Jellyfin Remote Control Integration and Session Visibility

**Status**:
- ✅ **Audio Playback Fixes**
  - Implemented support for multiple audio formats (S16, F32, U8, S24, S32)
  - Fixed format conversion using proper Symphonia API methods
  - Added graceful fallback for unsupported formats
  - Configured ALSA for proper integration with CamillaDSP (`hw:Loopback,0,0`)
  - Fixed "Resource busy" issues with proper device selection
  - Documented ALSA configuration options in the README

- ✅ **Documentation Updates**
  - Updated README with ALSA configuration details
  - Created comprehensive `lessons-learned.mdc` document for Jellyfin integration insights
  - Documented session management requirements for remote control functionality
  - Added error handling patterns for thread-safe operation

- ✅ **User Experience Improvements**
  - Implemented clean exit handling with Ctrl+C support
  - Added exit functionality at all navigation points
  - Fixed signal handling to ensure graceful shutdown
  - Ensured background tasks are properly terminated during exit

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

- ✅ **Jellyfin Remote Control State Reporting**
  - Implemented state reporting (`PlaybackStart`, `PlaybackStopped`, `ReportPlaybackProgress`) from `Player` -> `WebSocketHandler` -> Jellyfin Server.
  - Utilized Tokio MPSC channel (`PlayerStateUpdate`) for communication between `Player` and `WebSocketHandler`.
  - Used shared state (`Arc<StdMutex<PlaybackProgressInfo>>`) for `AlsaPlayer` to report playback time to `Player`.
  - Refactored `Player` to manage background tasks for playback and progress reporting.
  - Code compiles, but requires testing to confirm UI visibility and control in Jellyfin Web UI.

**Next Steps**:
1.  **Test State Reporting**: Verify that the client's playback state (start, stop, progress) is correctly reflected in the Jellyfin Web UI and that basic remote control commands (like stop) work.
2.  **Implement Remaining Playback Controls**: Add ALSA-level implementation for pause, seek, and volume control, and integrate them with the `Player` and WebSocket communication.
3.  **Address Other High-Priority Tasks**: Continue with tasks like real streaming/decoding improvements as per `docs/tasks_plan.md`.
4.  **Refine Documentation**: Update documentation based on testing results and further implementation.
