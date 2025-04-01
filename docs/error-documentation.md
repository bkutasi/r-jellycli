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
