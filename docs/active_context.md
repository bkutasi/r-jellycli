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

- ✅ **Jellyfin Remote Control Integration**
  - Implemented proper client capability reporting format to match Jellyfin API expectations
  - Updated session management to establish WebSocket connection immediately after capabilities reporting
  - Added session ID to WebSocket URL for proper remote control association
  - Implemented session keep-alive pings (every 30 seconds) to maintain session visibility
  - Fixed error handling and thread safety issues in the implementation
  - Added `DeviceId` field to `Settings` and ensured it's correctly generated/loaded and used for session reporting.
  - Improved WebSocket error handling for potentially more graceful closure.

- ✅ **SSDP Discovery Implementation**
  - Added an asynchronous SSDP broadcasting task (`src/ssdp/broadcaster.rs`).
  - Integrated the task into the main application flow (`src/main.rs`).
  - Added logging to the SSDP task for monitoring.
  - Corrected compilation errors related to SSDP integration and `DeviceId` handling.

- ❌ **Failed Simplification Attempt**: Removed SSDP broadcaster, HTTP pings, and refactored session/ID handling to align with `jellycli-repo`. This did *not* resolve the client visibility issue.

- 🚧 **Current Blockers**
  - Client still does not appear in Jellyfin's "play on" menu, even after the simplification attempt (removing SSDP, aligning session handling).

**Next Steps**: (Reverting to original debugging plan)
1.  Verify SSDP broadcast content and timing using network analysis tools (e.g., Wireshark).
2.  Perform deeper debugging of the WebSocket connection lifecycle: Why does it close prematurely? Are keep-alives consistently sent/received? Are incoming messages handled correctly?
3.  Compare network traffic (SSDP and WebSocket) between `r-jellycli` and `jellycli` to identify subtle differences in protocol implementation.
4.  Review Jellyfin server logs again with detailed client-side logging enabled (including SSDP and WebSocket traces).
5.  Consider adding handling for SSDP M-SEARCH requests if broadcasting alone is insufficient.
