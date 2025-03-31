# r-jellycli: Rust Jellyfin CLI Client

A command-line interface client for Jellyfin media server, written in Rust. This lightweight client allows you to browse and play media content from Jellyfin servers without a graphical user interface.

## Project Status

This project is in **early development**. Current implementation:

- Authentication with Jellyfin server (both API key and username/password)
- Basic media library browsing
- Directory navigation
- Basic audio playback via ALSA
- Configuration persistence (server URL, credentials, etc.)

## Features

- **Simple Command-Line Interface**: Navigate your media libraries with easy keyboard commands
- **Authentication**: Support for both API key and username/password authentication
- **Library Browsing**: Browse your Jellyfin media libraries, folders, and collections
- **Audio Playback**: Stream and play audio content using ALSA
- **Configuration Management**: Save your server URL, credentials, and playback preferences

## Requirements

- Rust toolchain (2021 edition or later)
- ALSA development libraries
- Active Jellyfin server (v10.x or later)
- Linux operating system (currently ALSA-dependent)

## Installation

### Prerequisites

Install the ALSA development libraries:

```bash
# Debian/Ubuntu
sudo apt-get install libasound2-dev

# Fedora
sudo dnf install alsa-lib-devel

# Arch Linux
sudo pacman -S alsa-lib
```

### Building from Source

```bash
# Clone the repository
git clone https://github.com/yourusername/r-jellycli.git
cd r-jellycli

# Build the project (optimized release build)
cargo build --release

# The executable will be available at
# ./target/release/r-jellycli
```

## Usage

### Basic Usage

```bash
# Run the client with a server URL and API key
./target/release/r-jellycli --server-url "http://your-jellyfin-server:8096" --api-key "your-api-key"

# Or use username/password authentication
./target/release/r-jellycli --server-url "http://your-jellyfin-server:8096" --username "your-username" --password "your-password"
```

### Configuration Options

All settings can be configured via either command line arguments or environment variables:

#### Command-line Options

| Option | Description |
|--------|-------------|
| `--server-url <URL>` | URL of your Jellyfin server |
| `--api-key <KEY>` | API key for authentication |
| `--username <USER>` | Username for authentication |
| `--password <PASS>` | Password for authentication |
| `--alsa-device <DEVICE>` | ALSA device to use for playback (default: "default") |
| `--config <PATH>` | Custom path to configuration file |

#### ALSA Audio Configuration

The application supports various ALSA devices for audio playback. Some recommended configurations:

| Device | Description | Sample Rate | Use Case |
|--------|-------------|-------------|-----------|
| `default` | System default device | Auto | General use |
| `hw:CARD=X,DEV=Y` | Direct hardware device | Fixed | Low latency |
| `hw:Loopback,0,0` | ALSA loopback subdevice | 48000Hz | Use with CamillaDSP |
| `plughw:CARD=X,DEV=Y` | Plugin hardware device | Auto | Format conversion |
| `dmix:CARD=X,DEV=Y` | Direct mixing device | Auto | Shared access |

When using with CamillaDSP or similar audio processing tools, set your ALSA device to match your DSP configuration:

```bash
# Example: Using with CamillaDSP on loopback device
ALSA_DEVICE="hw:Loopback,0,0" cargo run --bin r-jellycli
```

#### Audio Format Support

The application handles multiple audio formats and automatically converts them to the required format for ALSA output. Supported input formats:

- S16 (16-bit signed PCM)
- F32 (32-bit floating point)
- U8 (8-bit unsigned PCM)
- S24 (24-bit signed PCM)
- S32 (32-bit signed PCM)

Format conversion is handled internally with appropriate scaling and bit-depth conversion.

#### Environment Variables

| Variable | Description | Corresponding CLI Option |
|----------|-------------|--------------------------|
| `JELLYFIN_SERVER_URL` | Server URL | `--server-url` |
| `JELLYFIN_API_KEY` | API key | `--api-key` |
| `JELLYFIN_USERNAME` | Username | `--username` |
| `JELLYFIN_PASSWORD` | Password | `--password` |
| `ALSA_DEVICE` | ALSA device | `--alsa-device` |

Environment variables will be used if the corresponding command line argument is not provided.

### Interactive Commands

Once running, you can navigate using these commands:

- Select item: Enter the item number and press Enter
- Go back: Type `b` or `back` after playback
- Quit: Type `q` or `quit` after playback or at any navigation screen
- Exit anytime: Press `Ctrl+C` for graceful shutdown

The application now properly handles Ctrl+C interruptions at any point during execution, ensuring a clean shutdown even during media playback or while waiting for user input. This makes it easy to exit the application at any time without leaving orphaned processes or incomplete shutdown procedures.

## Configuration

The application stores configuration in `~/.config/jellycli/config.json` which contains:

- Server URL
- API key (if used)
- User ID
- ALSA device settings

You can specify a custom configuration path with the `--config` option.

## Testing

The project follows Rust's standard testing practices with a well-organized test structure. Here's how the test files are organized:

### Test Organization

- **Unit Tests**: Located within each module's source file using `#[cfg(test)]` module
- **Integration Tests**: Located in `tests/integration/` directory
- **Manual Test Utilities**: Located in `tests/manual/` directory
- **Common Test Utilities**: Located in `tests/test_utils.rs`

### Running Tests

#### Run All Tests

```bash
# Run all non-ignored tests
cargo test
```

#### Run Unit Tests Only

```bash
# Run unit tests only
cargo test --lib
```

#### Run Integration Tests

```bash
# Run integration tests
cargo test --test integration_tests
```

#### Run Specific Tests

```bash
# Run tests with names containing "config"
cargo test config
```

#### Run Ignored Tests

Some tests are marked with `#[ignore]` because they require external resources (like a running Jellyfin server):

```bash
# Run only the ignored tests
cargo test -- --ignored
```

### Manual Test Utilities

The project includes several manual test utilities for development and debugging:

```bash
# Test authentication against a Jellyfin server
cargo run --bin auth_test

# Test API endpoints
cargo run --bin api_test

# Test connection to a server
cargo run --bin test_connection
```

These manual tests require a valid `credentials.json` file in the project root with the following format:

```json
{
  "server_url": "http://your-jellyfin-server:8096",
  "username": "your-username",
  "password": "your-password"
}
```

## Development Roadmap

Planned features:

- Improved error handling
- Remote client interaction
- Better logging