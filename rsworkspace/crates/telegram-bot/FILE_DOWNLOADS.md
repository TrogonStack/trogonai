# File Download Functionality

This document describes how to download files from Telegram using the bot's file download API.

## Overview

The Telegram bot supports two file operations:
1. **Get File Info** - Retrieves file metadata and download URL from Telegram
2. **Download File** - Downloads a file from Telegram and saves it locally

## File ID Sources

File IDs can be obtained from various message types:
- **Photos**: `PhotoSize.file_id` from `MessagePhotoEvent`
- **Videos**: `FileInfo.file_id` from `MessageVideoEvent`
- **Audio**: `FileInfo.file_id` from `MessageAudioEvent`
- **Documents**: `FileInfo.file_id` from `MessageDocumentEvent`
- **Voice**: `FileInfo.file_id` from `MessageVoiceEvent`

## NATS Subjects

### Agent Commands (Agents → Bot)
- `telegram.{prefix}.agent.file.get` - Get file information
- `telegram.{prefix}.agent.file.download` - Download file

### Bot Responses (Bot → Agents)
- `telegram.{prefix}.bot.file.info` - File information response
- `telegram.{prefix}.bot.file.downloaded` - File download completion response

## Usage Examples

### 1. Get File Information

Send a `GetFileCommand` to retrieve file metadata:

```json
{
  "file_id": "AgACAgIAAxkBAAIBY2Z...",
  "request_id": "req-123"  // Optional tracking ID
}
```

**Response** on `telegram.{prefix}.bot.file.info`:

```json
{
  "file_id": "AgACAgIAAxkBAAIBY2Z...",
  "file_unique_id": "AQADeZoAAhvxWEl4",
  "file_size": 152048,
  "file_path": "photos/file_1.jpg",
  "download_url": "https://api.telegram.org/file/bot<token>/photos/file_1.jpg",
  "request_id": "req-123"
}
```

### 2. Download File

Send a `DownloadFileCommand` to download and save a file:

```json
{
  "file_id": "AgACAgIAAxkBAAIBY2Z...",
  "destination_path": "downloads/user_photo.jpg",
  "request_id": "req-456"  // Optional tracking ID
}
```

**Response** on `telegram.{prefix}.bot.file.downloaded`:

**Success:**
```json
{
  "file_id": "AgACAgIAAxkBAAIBY2Z...",
  "local_path": "downloads/user_photo.jpg",
  "file_size": 152048,
  "success": true,
  "error": null,
  "request_id": "req-456"
}
```

**Failure:**
```json
{
  "file_id": "AgACAgIAAxkBAAIBY2Z...",
  "local_path": "downloads/user_photo.jpg",
  "file_size": 0,
  "success": false,
  "error": "Failed to get file info: File not found",
  "request_id": "req-456"
}
```

## Implementation Details

### File Size Limits

Telegram has file size limits:
- **Photos**: Up to 10 MB
- **Videos**: Up to 50 MB (via bot API)
- **Documents**: Up to 50 MB (via bot API)
- **Audio/Voice**: Up to 50 MB

Files larger than these limits cannot be downloaded via the bot API.

### Download Process

1. Bot receives `GetFileCommand` or `DownloadFileCommand`
2. Bot calls Telegram's `getFile` API with the file_id
3. Telegram returns file metadata including `file_path`
4. For download requests:
   - Bot constructs download URL: `https://api.telegram.org/file/bot<token>/<file_path>`
   - Downloads file using HTTP GET request
   - Saves to specified `destination_path`
   - Creates parent directories if needed
   - Publishes response with success status

### Security Considerations

1. **Path Validation**: Ensure `destination_path` doesn't allow directory traversal attacks
2. **Storage Limits**: Monitor disk space usage for downloads
3. **Token Security**: Download URLs contain the bot token - handle with care
4. **File Permissions**: Downloaded files inherit default system permissions

### Error Handling

Common errors:
- **Invalid file_id**: File doesn't exist or has expired
- **File too large**: Exceeds Telegram's size limits
- **Network errors**: Connection issues during download
- **Disk errors**: Insufficient space or permission issues
- **Invalid path**: Parent directory doesn't exist (auto-created by implementation)

## Example Agent Workflow

### Downloading a User's Photo

```rust
// 1. Receive photo message event
let photo_event: MessagePhotoEvent = /* from NATS */;

// 2. Get largest photo size
let largest_photo = photo_event.photo.last().unwrap();

// 3. Request download
let download_cmd = DownloadFileCommand {
    file_id: largest_photo.file_id.clone(),
    destination_path: format!("user_photos/{}.jpg", photo_event.metadata.session_id),
    request_id: Some(photo_event.metadata.event_id.to_string()),
};

// 4. Publish download command
nats_client.publish(
    "telegram.prod.agent.file.download",
    serde_json::to_vec(&download_cmd)?
).await?;

// 5. Subscribe to download response
let response: FileDownloadResponse = /* from NATS */;

if response.success {
    println!("Photo downloaded to: {}", response.local_path);
} else {
    eprintln!("Download failed: {}", response.error.unwrap());
}
```

### Just Getting File Info

```rust
// 1. Get file info without downloading
let get_file_cmd = GetFileCommand {
    file_id: "AgACAgIAAxkBAAIBY2Z...".to_string(),
    request_id: Some("check-size-123".to_string()),
};

// 2. Publish command
nats_client.publish(
    "telegram.prod.agent.file.get",
    serde_json::to_vec(&get_file_cmd)?
).await?;

// 3. Receive response
let file_info: FileInfoResponse = /* from NATS */;

println!("File size: {} bytes", file_info.file_size.unwrap_or(0));
println!("Download URL: {}", file_info.download_url);

// 4. Optionally download using the URL directly
// (useful for streaming or external downloads)
```

## Best Practices

1. **Request IDs**: Use `request_id` to track async operations
2. **Error Handling**: Always check `success` field in download responses
3. **File Organization**: Use structured paths (e.g., by user_id, date, type)
4. **Cleanup**: Implement periodic cleanup of old downloaded files
5. **Monitoring**: Track download failures and success rates
6. **Rate Limiting**: Telegram API has rate limits - don't spam requests

## API Reference

### Commands

#### GetFileCommand
```rust
pub struct GetFileCommand {
    pub file_id: String,           // Required: Telegram file_id
    pub request_id: Option<String>, // Optional: Tracking ID
}
```

#### DownloadFileCommand
```rust
pub struct DownloadFileCommand {
    pub file_id: String,           // Required: Telegram file_id
    pub destination_path: String,   // Required: Local save path
    pub request_id: Option<String>, // Optional: Tracking ID
}
```

### Responses

#### FileInfoResponse
```rust
pub struct FileInfoResponse {
    pub file_id: String,            // Original file_id
    pub file_unique_id: String,     // Unique identifier
    pub file_size: Option<u64>,     // Size in bytes
    pub file_path: String,          // Path on Telegram servers
    pub download_url: String,       // Full download URL
    pub request_id: Option<String>, // Original request_id
}
```

#### FileDownloadResponse
```rust
pub struct FileDownloadResponse {
    pub file_id: String,            // Original file_id
    pub local_path: String,         // Where file was saved
    pub file_size: u64,             // Downloaded size in bytes
    pub success: bool,              // True if successful
    pub error: Option<String>,      // Error message if failed
    pub request_id: Option<String>, // Original request_id
}
```
