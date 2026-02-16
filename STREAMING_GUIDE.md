# Streaming Response System Guide

## Overview

The Telegram bot supports streaming responses, which allows AI-generated messages to be displayed progressively as they're being generated, similar to ChatGPT's streaming interface.

## How It Works

### 1. Configuration

Streaming is configured in `config/telegram-bot.toml`:

```toml
[telegram.features]
streaming = "partial"  # Options: "disabled", "partial", "full"
```

**Modes:**
- **`disabled`**: No streaming, send complete messages only
- **`partial`**: Stream long messages in chunks (recommended)
- **`full`**: Stream all messages progressively, even short ones

### 2. Architecture

```
Agent generates text → StreamMessageCommand → NATS → Bot → Telegram
       ↓                        ↓                            ↓
   Token by token          Chunk updates              Live editing
```

### 3. Message Flow

#### First Chunk (Initial Message)
```rust
StreamMessageCommand {
    chat_id: 123456,
    message_id: None,        // No message ID = create new message
    text: "Here's the beginning...",
    parse_mode: Some(ParseMode::Markdown),
    is_final: false,         // More chunks coming
}
```
**Bot action**: Send new message, get message ID

#### Subsequent Chunks (Updates)
```rust
StreamMessageCommand {
    chat_id: 123456,
    message_id: Some(4567), // Edit this message
    text: "Here's the beginning of an answer that keeps growing...",
    parse_mode: Some(ParseMode::Markdown),
    is_final: false,
}
```
**Bot action**: Edit existing message with updated text

#### Final Chunk
```rust
StreamMessageCommand {
    chat_id: 123456,
    message_id: Some(4567),
    text: "Here's the complete answer with all details!",
    parse_mode: Some(ParseMode::Markdown),
    is_final: true,          // This is the final version
}
```
**Bot action**: Final edit, no more updates coming

### 4. NATS Subject

Streaming commands are published to:
```
{prefix}.telegram.agent.message.stream
```

Example: `prod.telegram.agent.message.stream`

### 5. Implementation in Agent

To implement streaming from the agent side, you need to:

```rust
use telegram_types::commands::StreamMessageCommand;
use telegram_nats::{MessagePublisher, subjects};

async fn stream_response(
    publisher: &MessagePublisher,
    chat_id: i64,
    response_stream: impl Stream<Item = String>
) -> Result<()> {
    let mut message_id: Option<i32> = None;
    let mut accumulated_text = String::new();

    // Process each chunk from the LLM
    while let Some(chunk) = response_stream.next().await {
        accumulated_text.push_str(&chunk);

        // Send update every N characters or every M milliseconds
        if should_send_update(&accumulated_text) {
            let command = StreamMessageCommand {
                chat_id,
                message_id,
                text: accumulated_text.clone(),
                parse_mode: Some(ParseMode::Markdown),
                is_final: false,
            };

            let subject = subjects::agent::message_stream(publisher.prefix());

            // Publish and get message ID from first response
            if message_id.is_none() {
                // First message - the bot will send and return message ID
                // (Note: current implementation doesn't return ID, needs enhancement)
                message_id = Some(/* get from response */);
            }

            publisher.publish(&subject, &command).await?;
        }
    }

    // Send final update
    let command = StreamMessageCommand {
        chat_id,
        message_id,
        text: accumulated_text,
        parse_mode: Some(ParseMode::Markdown),
        is_final: true,
    };

    let subject = subjects::agent::message_stream(publisher.prefix());
    publisher.publish(&subject, &command).await?;

    Ok(())
}
```

### 6. Update Strategies

#### Option A: Character-based
Update every N characters (e.g., every 50-100 characters):
```rust
fn should_send_update(text: &str) -> bool {
    text.len() % 100 == 0
}
```

#### Option B: Time-based
Update every N milliseconds:
```rust
let mut last_update = Instant::now();
fn should_send_update() -> bool {
    last_update.elapsed() > Duration::from_millis(500)
}
```

#### Option C: Word boundary-based
Update at word boundaries to avoid cutting words:
```rust
fn should_send_update(text: &str, last_len: usize) -> bool {
    let new_chars = text.len() - last_len;
    new_chars > 50 && text.ends_with(|c: char| c.is_whitespace())
}
```

### 7. Best Practices

1. **Rate Limiting**: Telegram has edit rate limits (~1 edit/second per message)
   - Don't update too frequently
   - Buffer updates if needed

2. **Message ID Tracking**:
   - Store the message ID after first send
   - Use it for all subsequent edits
   - Currently needs to be tracked manually

3. **Error Handling**:
   - If edit fails, log but don't crash
   - Consider falling back to non-streaming

4. **Graceful Degradation**:
   - If streaming disabled, send complete message only
   - Check config before streaming

5. **Visual Indicators**:
   - Add "..." or "⏳" to show streaming in progress
   - Remove on final message

### 8. Telegram Limitations

- **Edit frequency**: ~1 edit per second per message
- **Message length**: 4096 characters max
- **Edit timeout**: Messages can be edited for 48 hours after sending

### 9. Example: LLM Integration with Streaming

```rust
async fn process_with_streaming(
    &self,
    chat_id: i64,
    user_message: &str,
    publisher: &MessagePublisher,
) -> Result<()> {
    // Get streaming response from Claude API
    let mut stream = self.llm_client.stream_response(user_message).await?;

    let mut message_id: Option<i32> = None;
    let mut buffer = String::new();
    let mut last_update = Instant::now();

    while let Some(chunk) = stream.next().await {
        buffer.push_str(&chunk);

        // Update every 500ms or every 100 characters
        if last_update.elapsed() > Duration::from_millis(500)
           || buffer.len() % 100 == 0 {

            let command = StreamMessageCommand {
                chat_id,
                message_id,
                text: buffer.clone(),
                parse_mode: Some(ParseMode::Markdown),
                is_final: false,
            };

            publisher.publish(
                &subjects::agent::message_stream(publisher.prefix()),
                &command
            ).await?;

            last_update = Instant::now();
        }
    }

    // Final message
    let command = StreamMessageCommand {
        chat_id,
        message_id,
        text: buffer,
        parse_mode: Some(ParseMode::Markdown),
        is_final: true,
    };

    publisher.publish(
        &subjects::agent::message_stream(publisher.prefix()),
        &command
    ).await?;

    Ok(())
}
```

### 10. Current Limitations

1. **Message ID not returned**: The bot doesn't currently return the message ID after sending
   - Need to implement response channel or use request-reply pattern

2. **No buffering**: Updates are sent immediately
   - Could add smart buffering to respect rate limits

3. **No retry logic**: Failed edits are just logged
   - Could implement retry with backoff

### 11. Future Enhancements

- [ ] Request-reply pattern to get message ID
- [ ] Smart buffering to respect rate limits
- [ ] Automatic retry with exponential backoff
- [ ] Stream cancellation support
- [ ] Progress indicators in UI
- [ ] Typing indicator during streaming

## Summary

The streaming system is **already implemented** in the bot infrastructure:
- ✅ `StreamMessageCommand` type defined
- ✅ NATS subject configured
- ✅ Bot handler processes stream messages
- ✅ Supports both new messages and edits
- ✅ `is_final` flag for completion

**What's needed**: Agent-side implementation to:
1. Break LLM response into chunks
2. Publish `StreamMessageCommand` updates
3. Track message ID for edits
4. Send final update with `is_final: true`
