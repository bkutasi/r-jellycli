# Active Development Context

**Current Focus**: Core Functionality Refinement & Documentation

**Status**:

- ✅ **Jellyfin Remote Control Implementation & Fixes**
  - Implemented handling for `PlayNow` command: Correctly stops existing playback and starts the new track/queue. This involved careful management of the `PlaybackOrchestrator` task and ALSA resources to ensure the previous track stopped fully before the new one began.
  - Implemented handling for `Stop` command: Triggers a graceful application shutdown, similar to Ctrl+C.
  - Resolved issue where subsequent `PlayNow` commands failed due to improper task/resource cleanup.
  - Fixed application hang during shutdown (Ctrl+C or remote `Stop`) by ensuring all tasks (playback, reporter, WebSocket listener) and the ALSA device close cleanly with appropriate timeouts and synchronization.
  - Corrected capabilities reporting: Removed non-standard commands (like "Stop") and unnecessary volume controls (`SetVolume`, etc.) to prevent HTTP 400 errors from the server.
  - Ensured correct ALSA device (specified at startup) is used, not the system default.

- ✅ **Audio Playback Functional**
  - Core pipeline (decode, resample, write) is functional.
  - Playback state reporting via HTTP POST is functional.
  - ALSA underrun handling is implemented.

- ✅ **Stable WebSocket & Session Management**
  - Persistent WebSocket connection established and maintained.
  - Client appears reliably in Jellyfin's "Play On" menu.
  - Correct `DeviceId` handling.

- ✅ **Clean Exit & Refactoring**
  - Graceful shutdown via Ctrl+C and remote `Stop` command.
  - Core components (`audio`, `jellyfin`) refactored for better modularity and maintainability.

**Current State Summary**:
- Core audio playback is functional.
- Remote control commands `PlayNow` and `Stop` are implemented and working correctly.
- Application shutdown is stable and graceful.
- WebSocket connection and session reporting are stable.
- Capabilities reporting is corrected.

**Next Steps**:
1.  **Implement Remaining Playback Controls**: Add ALSA-level implementation for pause, seek, and volume control, and integrate them with the `Player` and HTTP/WebSocket communication as needed.
2.  **Refine Buffering/Error Handling**: Improve robustness of the audio streaming and playback pipeline.
3.  **Address Other Tasks**: Continue with tasks as per `docs/tasks_plan.md`.
4.  **Documentation**: Continue updating documentation to reflect the current state.
