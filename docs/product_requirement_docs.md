# Product Requirements Document

## Project Overview

**Project Name:** r-jellycli (Jellyfin CLI Client)

**Purpose:** A command-line interface client for Jellyfin media server, written in Rust, allowing users to access and play media content from Jellyfin servers without a graphical interface.

## Target Users

- Jellyfin server administrators
- Command-line power users
- Users of headless systems
- Users who prefer lightweight applications for media playback

## Core Requirements

### Functional Requirements

1. **Authentication & Connection**
   - Connect to Jellyfin server via URL and API key
   - Support for secure connections (HTTPS)

3. **Media Playback**
   - Play audio files through ALSA
   - Support common audio formats (MP3, FLAC, OGG, etc.)
   - Basic playback controls (play, pause, stop, skip)
   - Remote control thought other client with WebSocker ("Play on" on the web interface)

4. **Logging & Others**
   - Simple, intuitive, informative logs
   - Clear error messages and feedback
   - Configuration file support for persistent settings

### Non-Functional Requirements

1. **Performance**
   - Low resource usage
   - Fast response times even on low-end hardware
   - Efficient streaming with minimal buffering

2. **Reliability**
   - Graceful error handling
   - Stable playback without interruptions
   - Automatic reconnection on network issues

3. **Compatibility**
   - Support for Linux systems (primary)
   - Cross-platform compatibility where possible
   - Compatibility with different versions of Jellyfin server

## Constraints

- Must work with minimal dependencies
- Should maintain a small footprint
- Must be accessible through standard terminals
