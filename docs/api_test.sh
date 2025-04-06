#!/bin/bash

# This script automates testing the core Jellyfin API sequence:
# Authenticate -> Report Capabilities -> Establish WebSocket -> Fetch Data
# Note: The WebSocket step (Step 3) is skipped as curl cannot maintain WebSocket connections.

# --- Configuration ---
JELLYFIN_SERVER_URL="http://192.168.1.178:8096"
JELLYFIN_USERNAME="bkutasi"
# Use single quotes to handle special characters in the password
JELLYFIN_PASSWORD='7FpR>^ceVmM.uFc$kwAz'

# --- Helper Function for Confirmation ---
confirm_or_exit() {
    local prompt_message=$1
    echo "" # Add a blank line for spacing
    read -p "$prompt_message Press Enter to continue, or 'q' then Enter to quit: " user_input
    if [[ "$user_input" == "q" || "$user_input" == "Q" ]]; then
        echo "Exiting script."
        exit 0
    fi
    echo "" # Add a blank line for spacing
}

# --- Dependency Check ---
echo "Checking for required commands (curl, jq)..."
if ! command -v curl &> /dev/null; then
    echo "Error: curl command not found. Please install curl."
    exit 1
fi
if ! command -v jq &> /dev/null; then
    echo "Error: jq command not found. Please install jq."
    exit 1
fi
echo "Dependencies found."

confirm_or_exit "After dependency check."

# --- Step 1: Authenticate User ---
echo "Step 1: Authenticating user (Authenticate)..."
AUTH_PAYLOAD=$(cat <<EOF
{
  "Username": "$JELLYFIN_USERNAME",
  "Pw": "$JELLYFIN_PASSWORD"
}
EOF
)

# Use -s for silent, capture response and http_code separately
# Create a temporary file for the response body
RESPONSE_FILE=$(mktemp)
trap 'rm -f "$RESPONSE_FILE"' EXIT # Ensure temp file is deleted on exit

http_code=$(curl -s -w "%{http_code}" -o "$RESPONSE_FILE" -X POST "$JELLYFIN_SERVER_URL/Users/AuthenticateByName" \
  -H 'Content-Type: application/json' \
  -d "$AUTH_PAYLOAD")

if [ "$http_code" -ne 200 ]; then
    echo "Error: Authentication failed. HTTP status code: $http_code"
    echo "Response:"
    cat "$RESPONSE_FILE" # Show error response if any
    exit 1
fi

# Extract AccessToken and UserId using jq
ACCESS_TOKEN=$(jq -r '.AccessToken' "$RESPONSE_FILE")
USER_ID=$(jq -r '.User.Id' "$RESPONSE_FILE")

# Validate extracted values
if [ -z "$ACCESS_TOKEN" ] || [ "$ACCESS_TOKEN" == "null" ]; then
    echo "Error: Failed to extract AccessToken from authentication response."
    echo "Response:"
    cat "$RESPONSE_FILE"
    exit 1
fi
if [ -z "$USER_ID" ] || [ "$USER_ID" == "null" ]; then
    echo "Error: Failed to extract UserId from authentication response."
    echo "Response:"
    cat "$RESPONSE_FILE"
    exit 1
fi
echo "Authentication successful. UserID: $USER_ID"
# echo "AccessToken: $ACCESS_TOKEN" # Optionally hide token in output

confirm_or_exit "After Step 1 (Authenticate)."

# --- Step 2: Register Client Capabilities ---
echo "Step 2: Registering client capabilities (Report Capabilities)..."
CAPABILITIES_PAYLOAD=$(cat <<EOF
{
  "capabilities": {
    "PlayableMediaTypes": [ "Audio" ],
    "SupportedCommands": [ "PlayState", "Play", "Seek", "NextTrack", "PreviousTrack", "SetShuffleQueue", "SetRepeatMode", "PlayNext" ],
    "SupportsMediaControl": true,
    "SupportsPersistentIdentifier": false
  }
}
EOF
)

http_code=$(curl -s -o /dev/null -w "%{http_code}" -X POST "$JELLYFIN_SERVER_URL/Sessions/Capabilities/Full" \
  -H "X-Emby-Token: $ACCESS_TOKEN" \
  -H 'Content-Type: application/json' \
  -d "$CAPABILITIES_PAYLOAD")

if [ "$http_code" -ne 204 ]; then
    echo "Error: Failed to register client capabilities. HTTP status code: $http_code"
    exit 1
fi
echo "Client capabilities registered successfully (HTTP 204)."

confirm_or_exit "After Step 2 (Report Capabilities)."

# --- Step 3: Establish WebSocket Connection (Skipped) ---
echo "Step 3: Establish WebSocket Connection (Skipped - requires dedicated client)."
# This step is part of the sequence but cannot be automated with curl.
# Manual test requires a tool like wscat or Postman's WebSocket feature.
# URL: ws://$JELLYFIN_SERVER_URL/socket?api_key=$ACCESS_TOKEN&deviceId=test-script-device-`uuidgen`

confirm_or_exit "After Step 3 (Establish WebSocket - Skipped)."

# --- Step 4: Fetch User Views ---
echo "Step 4: Fetching user views (Fetch Data - Views)..."
http_code=$(curl -s -o /dev/null -w "%{http_code}" -X GET "$JELLYFIN_SERVER_URL/Users/$USER_ID/Views" \
  -H "X-Emby-Token: $ACCESS_TOKEN" \
  -H 'Accept: application/json')

if [ "$http_code" -ne 200 ]; then
    echo "Error: Failed to fetch user views. HTTP status code: $http_code"
    exit 1
fi
echo "User views fetched successfully (HTTP 200)."

confirm_or_exit "After Step 4 (Fetch Views)."

# --- Step 5: Fetch User Items (Top-Level) ---
echo "Step 5: Fetching user items (Fetch Data - Items)..."
http_code=$(curl -s -o /dev/null -w "%{http_code}" -X GET "$JELLYFIN_SERVER_URL/Users/$USER_ID/Items" \
  -H "X-Emby-Token: $ACCESS_TOKEN" \
  -H 'Accept: application/json')

if [ "$http_code" -ne 200 ]; then
    echo "Error: Failed to fetch user items. HTTP status code: $http_code"
    exit 1
fi
echo "User items fetched successfully (HTTP 200)."

echo ""
echo "API test sequence (excluding WebSocket) completed successfully."
exit 0