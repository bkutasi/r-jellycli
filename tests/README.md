# Testing Documentation for r-jellycli

This directory contains all tests for the r-jellycli project. The tests are organized according to Rust best practices to ensure proper test coverage and maintainability.

## Test Structure

The test structure follows standard Rust conventions:

### Unit Tests

- Located within the source files using the `#[cfg(test)]` attribute
- Focus on testing individual components in isolation
- Each module contains its own unit tests in a `tests` submodule

### Integration Tests

- Located in the `tests/integration/` directory
- Focus on testing how components work together
- Entry point: `tests/integration_tests.rs`

### Manual Tests

- Located in the `tests/manual/` directory
- Command-line utilities for manual testing and debugging
- Require running explicitly with `cargo run --bin <test_name>`
- Often require external resources (Jellyfin server, test credentials)

## Running the Tests

### Run All Unit and Integration Tests

```bash
cargo test
```

### Run Only Unit Tests

```bash
cargo test --lib
```

### Run Only Integration Tests

```bash
cargo test --test integration_tests
```

### Run Ignored Tests (requires external resources)

```bash
cargo test -- --ignored
```

### Run Manual Test Utilities

```bash
# For the auth test utility
cargo run --bin auth_test

# For the API test utility
cargo run --bin api_test
```

## Test Utilities

Common test utilities are available in `tests/test_utils.rs` and include:

- Credential loading and management
- Mock clients and objects 
- Common test constants

## Adding New Tests

When adding new tests, follow these guidelines:

1. **Unit Tests:** Add within the source file in a `#[cfg(test)]` module
2. **Integration Tests:** Create a new file in `tests/integration/` and update `mod.rs`
3. **Manual Tests:** Add new utilities in `tests/manual/` and register them in `Cargo.toml`

## Test Credentials

The manual tests require a `credentials.json` file with the following structure:

```json
{
  "username": "your_jellyfin_username",
  "password": "your_jellyfin_password",
  "server_url": "http://your_jellyfin_server:8096"
}
```

**Important:** Never commit real credentials to version control. The `credentials.json` file is listed in `.gitignore`.
