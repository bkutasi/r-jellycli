# Manual API Test Procedure: Authentication, Session Initialization, and Data Fetching

This document outlines the steps to manually test the core API interaction sequence: **Authenticate -> Report Capabilities -> Establish WebSocket -> Fetch Data**. These steps involve authentication, session setup, WebSocket connection, and data retrieval, typically executed using tools like `curl` for HTTP requests and a dedicated WebSocket client for WebSocket connections.

**Target Base URL:** `http://192.168.1.178:8096` (Replace with the actual target server address if different)
**Target User ID:** `b109d0da6d654e1fb9949bb0e5a8101d` (Replace if testing with a different user)

---

## Test Steps

**Important:** Before proceeding, you need an `AccessToken` (API Key/Token). Store the obtained token securely (e.g., in an environment variable) for use in subsequent steps.

1.  **Authenticate User (POST)**

    *   **Purpose:** Obtain an `AccessToken` required for authenticating subsequent API calls.
    *   **Method:** POST
    *   **URL:** `http://192.168.1.178:8096/Users/AuthenticateByName`
    *   **Body (JSON):**
        ```json
        {
          "Username": "YOUR_JELLYFIN_USERNAME",
          "Pw": "YOUR_JELLYFIN_PASSWORD"
        }
        ```
    *   **Example `curl` Command:**
        ```bash
        # IMPORTANT: Replace YOUR_JELLYFIN_USERNAME and YOUR_JELLYFIN_PASSWORD
        # with your actual credentials. Avoid hardcoding these in scripts.
        # Consider using environment variables or a secure prompt method.
        curl -X POST 'http://192.168.1.178:8096/Users/AuthenticateByName' \
        -H 'Content-Type: application/json' \
        -d '{ "Username": "YOUR_JELLYFIN_USERNAME", "Pw": "YOUR_JELLYFIN_PASSWORD" }' \
        --verbose
        ```
    *   **Expected Outcome:** HTTP Status Code `200 OK` and a JSON response body containing user details and the crucial `AccessToken`. Extract and save the `AccessToken` value.

2.  **Register Client Capabilities (POST)**

    *   **Purpose:** Inform the server about the client's playback capabilities.
    *   **Method:** POST
    *   **URL:** `http://192.168.1.178:8096/Sessions/Capabilities/Full`
    *   **Header:** `X-Emby-Token: YOUR_ACCESS_TOKEN` (Replace `YOUR_ACCESS_TOKEN` with the token from Step 1)
    *   **Body (JSON):**
        ```json
        {
          "capabilities": {
            "PlayableMediaTypes": [
              "Audio"
            ],
            "SupportedCommands": [
              "PlayState",
              "Play",
              "Seek",
              "NextTrack",
              "PreviousTrack",
              "SetShuffleQueue",
              "SetRepeatMode",
              "PlayNext"
            ],
            "SupportsMediaControl": true,
            "SupportsPersistentIdentifier": false
          }
        }
        ```
    *   **Example `curl` Command:**
        ```bash
        # Replace YOUR_ACCESS_TOKEN with the token obtained in Step 1
        export JELLYFIN_TOKEN="YOUR_ACCESS_TOKEN"

        curl -X POST 'http://192.168.1.178:8096/Sessions/Capabilities/Full' \
        -H "X-Emby-Token: $JELLYFIN_TOKEN" \
        -H 'Content-Type: application/json' \
        -d '{ "capabilities": { "PlayableMediaTypes": [ "Audio" ], "SupportedCommands": [ "PlayState", "Play", "Seek", "NextTrack", "PreviousTrack", "SetShuffleQueue", "SetRepeatMode", "PlayNext" ], "SupportsMediaControl": true, "SupportsPersistentIdentifier": false } }' \
        --verbose
        ```
    *   **Expected Outcome:** HTTP Status Code `204 No Content`.

3.  **Establish WebSocket Connection (GET - Upgrade)**

    *   **Purpose:** Upgrade the HTTP connection to a persistent WebSocket connection for real-time communication.
    *   **Method:** GET (with upgrade headers)
    *   **URL:** `ws://192.168.1.178:8096/socket?api_key=YOUR_ACCESS_TOKEN&deviceId=YOUR_DEVICE_ID`
        *   **Note:** Replace `YOUR_ACCESS_TOKEN` with the token from Step 1. Replace `YOUR_DEVICE_ID` with a unique identifier for this test client (e.g., a generated UUID like `8f5ba2f6-1aa7-4172-880d-57082b0f0f2b`). The `api_key` parameter in the URL is the primary way to authenticate WebSocket connections.
    *   **Manual Testing Approach:**
        *   Standard HTTP tools like `curl` cannot maintain a WebSocket connection after the initial handshake.
        *   Use a dedicated WebSocket client tool (e.g., Postman's WebSocket feature, `wscat`, browser developer tools console, or specialized GUI tools).
        *   Input the WebSocket URL (`ws://...`) into the client, ensuring the correct `api_key` (your `AccessToken`) and a `deviceId` are included.
        *   Observe if the connection is successfully established (often indicated by the tool).
    *   **Expected Outcome:** Successful WebSocket connection establishment (HTTP Status `101 Switching Protocols` during the handshake, followed by an open connection).

4.  **Fetch User Views (GET)**

    *   **Purpose:** Retrieve the available library views (e.g., Music, Movies) for the specified user.
    *   **Method:** GET
    *   **URL:** `http://192.168.1.178:8096/Users/b109d0da6d654e1fb9949bb0e5a8101d/Views`
    *   **Header:** `X-Emby-Token: YOUR_ACCESS_TOKEN` (Replace `YOUR_ACCESS_TOKEN` with the token from Step 1)
    *   **Example `curl` Command:**
        ```bash
        # Ensure JELLYFIN_TOKEN is set from Step 1
        # export JELLYFIN_TOKEN="YOUR_ACCESS_TOKEN"

        curl -X GET 'http://192.168.1.178:8096/Users/b109d0da6d654e1fb9949bb0e5a8101d/Views' \
        -H "X-Emby-Token: $JELLYFIN_TOKEN" \
        -H 'Accept: application/json' \
        --verbose
        ```
    *   **Expected Outcome:** HTTP Status Code `200 OK` and a JSON response body containing a list of user views.

5.  **Fetch User Items (GET)**

    *   **Purpose:** Retrieve items within a specific view or folder for the user.
    *   **Method:** GET
    *   **URL:** `http://192.168.1.178:8096/Users/b109d0da6d654e1fb9949bb0e5a8101d/Items`
    *   **Header:** `X-Emby-Token: YOUR_ACCESS_TOKEN` (Replace `YOUR_ACCESS_TOKEN` with the token from Step 1)
    *   **Important Note on Parameters:** The base URL alone might return top-level items. To fetch items within a specific library (like 'Music'), you likely need to add query parameters. The most common parameter is `ParentId`.
        *   **Example `ParentId`:** `7e64e319657a9516ec78490da03edccb` (This ID would typically be obtained from the response in Step 4).
    *   **Example `curl` Command (with potential `ParentId`):**
        ```bash
        # Ensure JELLYFIN_TOKEN is set from Step 1
        # export JELLYFIN_TOKEN="YOUR_ACCESS_TOKEN"
        # Replace YOUR_PARENT_ID with the actual ID from Step 4 results
        export PARENT_ID="YOUR_PARENT_ID"

        curl -X GET "http://192.168.1.178:8096/Users/b109d0da6d654e1fb9949bb0e5a8101d/Items?ParentId=$PARENT_ID" \
        -H "X-Emby-Token: $JELLYFIN_TOKEN" \
        -H 'Accept: application/json' \
        --verbose
        ```
    *   **Example `curl` Command (without `ParentId` - for top-level items):**
        ```bash
        # Ensure JELLYFIN_TOKEN is set from Step 1
        # export JELLYFIN_TOKEN="YOUR_ACCESS_TOKEN"

        curl -X GET 'http://192.168.1.178:8096/Users/b109d0da6d654e1fb9949bb0e5a8101d/Items' \
        -H "X-Emby-Token: $JELLYFIN_TOKEN" \
        -H 'Accept: application/json' \
        --verbose
        ```
    *   **Expected Outcome:** HTTP Status Code `200 OK` and a JSON response body containing a list of items (songs, albums, folders, etc.) corresponding to the request parameters.

---