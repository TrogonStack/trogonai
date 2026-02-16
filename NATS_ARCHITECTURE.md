# NATS Architecture - TrogonAI Telegram Integration

## Subject Naming Pattern

```
telegram.{prefix}.{direction}.{entity}.{action}
```

- **prefix**: Environment identifier (e.g., `prod`, `dev`, `test`)
- **direction**: `bot` (Telegram → Agent) or `agent` (Agent → Telegram)
- **entity**: Type of entity (e.g., `message`, `callback`, `chat`)
- **action**: Specific action (e.g., `send`, `edit`, `text`, `photo`)

---

## 📥 Bot Events (Telegram → Agents)

Events published by the Telegram bot when users interact with it.

### Message Events

| Subject | Event Type | Description |
|---------|------------|-------------|
| `telegram.{prefix}.bot.message.text` | `MessageTextEvent` | User sent text message |
| `telegram.{prefix}.bot.message.photo` | `MessagePhotoEvent` | User sent photo |
| `telegram.{prefix}.bot.message.video` | `MessageVideoEvent` | User sent video |
| `telegram.{prefix}.bot.message.audio` | `MessageAudioEvent` | User sent audio |
| `telegram.{prefix}.bot.message.document` | `MessageDocumentEvent` | User sent document |
| `telegram.{prefix}.bot.message.voice` | `MessageVoiceEvent` | User sent voice message |

### Interaction Events

| Subject | Event Type | Description |
|---------|------------|-------------|
| `telegram.{prefix}.bot.callback.query` | `CallbackQueryEvent` | User clicked inline button |
| `telegram.{prefix}.bot.command.{name}` | `CommandEvent` | User sent command (e.g., `/start`) |

### Wildcards

```rust
telegram.{prefix}.bot.>              // All bot events
telegram.{prefix}.bot.message.>      // All message events
telegram.{prefix}.bot.command.>      // All command events
```

---

## 📤 Agent Commands (Agents → Telegram)

Commands sent by agents to control the Telegram bot.

### Message Commands

| Subject | Command Type | Description |
|---------|--------------|-------------|
| `telegram.{prefix}.agent.message.send` | `SendMessageCommand` | Send new text message |
| `telegram.{prefix}.agent.message.edit` | `EditMessageCommand` | Edit existing message |
| `telegram.{prefix}.agent.message.delete` | `DeleteMessageCommand` | Delete message |
| `telegram.{prefix}.agent.message.send_photo` | `SendPhotoCommand` | Send photo with caption |
| `telegram.{prefix}.agent.message.stream` | `StreamMessageCommand` | Stream message progressively |

### Interaction Commands

| Subject | Command Type | Description |
|---------|--------------|-------------|
| `telegram.{prefix}.agent.callback.answer` | `AnswerCallbackCommand` | Answer button click |
| `telegram.{prefix}.agent.chat.action` | `SendChatActionCommand` | Show typing indicator, etc. |

### Wildcards

```rust
telegram.{prefix}.agent.>            // All agent commands
```

---

## 📋 Data Structures

### Bot Events (Telegram → Agents)

#### MessageTextEvent
```json
{
  "metadata": {
    "event_id": "uuid-v4",
    "session_id": "tg-private-123456",
    "timestamp": "2024-02-16T20:30:00Z",
    "update_id": 123456789
  },
  "message": {
    "message_id": 42,
    "from": {
      "id": 123456,
      "username": "johndoe",
      "first_name": "John",
      "last_name": "Doe"
    },
    "chat": {
      "id": 123456,
      "type": "private",
      "username": "johndoe"
    },
    "date": 1708115400,
    "text": "Hello bot!"
  },
  "text": "Hello bot!"
}
```

#### MessagePhotoEvent
```json
{
  "metadata": { /* EventMetadata */ },
  "message": { /* Message */ },
  "photo": [
    {
      "file_id": "AgACAgIAAxkBAAI...",
      "file_unique_id": "AQAD...",
      "width": 1280,
      "height": 720,
      "file_size": 123456
    }
  ],
  "caption": "Check out this photo!"
}
```

#### CallbackQueryEvent
```json
{
  "metadata": { /* EventMetadata */ },
  "callback_query_id": "1234567890123456789",
  "from": { /* User */ },
  "chat": { /* Chat */ },
  "message_id": 42,
  "data": "button_action_confirm"
}
```

### Agent Commands (Agents → Telegram)

#### SendMessageCommand
```json
{
  "chat_id": 123456,
  "text": "Hello from the agent!",
  "parse_mode": "Markdown",
  "reply_to_message_id": 42,
  "reply_markup": {
    "inline_keyboard": [
      [
        {
          "text": "Confirm",
          "callback_data": "confirm"
        },
        {
          "text": "Cancel",
          "callback_data": "cancel"
        }
      ]
    ]
  }
}
```

#### EditMessageCommand
```json
{
  "chat_id": 123456,
  "message_id": 42,
  "text": "Updated message text",
  "parse_mode": "Markdown",
  "reply_markup": { /* Optional InlineKeyboardMarkup */ }
}
```

#### StreamMessageCommand
```json
{
  "chat_id": 123456,
  "message_id": null,
  "text": "This is a streaming response...",
  "parse_mode": "Markdown",
  "is_final": false
}
```

**Streaming Flow:**
1. First chunk: `message_id = null` → Creates new message
2. Subsequent chunks: Bot tracks `message_id` internally → Edits existing message
3. Final chunk: `is_final = true` → Cleanup tracking

#### SendChatActionCommand
```json
{
  "chat_id": 123456,
  "action": "typing"
}
```

**Available actions:**
- `typing`
- `upload_photo`
- `record_video`
- `upload_video`
- `record_voice`
- `upload_voice`
- `upload_document`
- `choose_sticker`
- `find_location`

---

## 🔄 Message Flow Examples

### Simple Text Interaction

```
1. User sends message → Telegram API → Bot
2. Bot publishes: telegram.prod.bot.message.text
   Data: MessageTextEvent

3. Agent receives event, processes, generates response
4. Agent publishes: telegram.prod.agent.message.send
   Data: SendMessageCommand

5. Bot receives command → Telegram API → User sees message
```

### Streaming Response

```
1. User: "Explain quantum physics"
2. Bot → NATS: telegram.prod.bot.message.text

3. Agent starts generating response:
   - Chunk 1: "Quantum physics is..." → telegram.prod.agent.message.stream
     {message_id: null, is_final: false}

   - Chunk 2: "Quantum physics is the study of..." → telegram.prod.agent.message.stream
     {message_id: null, is_final: false}  // Bot tracks internally

   - Chunk 3: "Quantum physics is the study of matter and energy at..." → telegram.prod.agent.message.stream
     {message_id: null, is_final: true}

4. Bot creates message on chunk 1, edits on chunks 2-3 (rate limited to 1 edit/sec)
5. After final chunk, cleanup tracking
```

### Button Interaction

```
1. Agent sends message with buttons:
   telegram.prod.agent.message.send
   {
     "text": "Choose an option:",
     "reply_markup": {
       "inline_keyboard": [[
         {"text": "Yes", "callback_data": "yes"},
         {"text": "No", "callback_data": "no"}
       ]]
     }
   }

2. User clicks button → Bot publishes:
   telegram.prod.bot.callback.query
   {"data": "yes", ...}

3. Agent processes callback, sends acknowledgment:
   telegram.prod.agent.callback.answer
   {"callback_query_id": "...", "text": "Processing..."}

4. Agent updates message:
   telegram.prod.agent.message.edit
   {"text": "You chose: Yes"}
```

---

## 🎯 Session ID Format

Session IDs identify unique conversation contexts:

```
tg-{chat_type}-{chat_id}[-{user_id}]
```

**Examples:**
- Private chat: `tg-private-123456`
- Group chat: `tg-group--987654321-123456` (group_id is negative, includes user_id)
- Channel: `tg-channel--100123456789`

**Purpose:**
- Track conversation history per session
- Manage streaming message state
- Isolate user contexts in multi-user environments

---

## 📊 Payload Sizes

| Type | Max Size | Notes |
|------|----------|-------|
| Text message | 4096 chars | Telegram API limit |
| Caption | 1024 chars | For photos, videos, etc. |
| Callback data | 64 bytes | Button callback payload |
| Command args | ~4000 chars | Part of message limit |

---

## 🔐 Common Fields

### ParseMode Enum
```rust
"Markdown" | "MarkdownV2" | "HTML"
```

### ChatAction Enum
```rust
"typing" | "upload_photo" | "record_video" | "upload_video" |
"record_voice" | "upload_voice" | "upload_document" |
"choose_sticker" | "find_location"
```

### InlineKeyboardMarkup
```json
{
  "inline_keyboard": [
    [
      {
        "text": "Button label",
        "callback_data": "action_id"
      },
      {
        "text": "URL Button",
        "url": "https://example.com"
      }
    ]
  ]
}
```

---

## 🚀 Usage Examples

### Publishing an Event (Bot)

```rust
use telegram_nats::{MessagePublisher, subjects};
use telegram_types::events::MessageTextEvent;

let publisher = MessagePublisher::new(nats_client, "prod".to_string());
let event = MessageTextEvent { /* ... */ };

publisher.publish(
    &subjects::bot::message_text("prod"),
    &event
).await?;
```

### Subscribing to Events (Agent)

```rust
use telegram_nats::{MessageSubscriber, subjects};
use telegram_types::events::MessageTextEvent;

let subscriber = MessageSubscriber::new(nats_client, "prod".to_string());
let mut stream = subscriber
    .subscribe::<MessageTextEvent>(&subjects::bot::message_text("prod"))
    .await?;

while let Some(result) = stream.next().await {
    match result {
        Ok(event) => {
            println!("Received: {}", event.text);
        }
        Err(e) => eprintln!("Error: {}", e),
    }
}
```

### Sending a Command (Agent)

```rust
use telegram_nats::{MessagePublisher, subjects};
use telegram_types::commands::{SendMessageCommand, ParseMode};

let publisher = MessagePublisher::new(nats_client, "prod".to_string());
let command = SendMessageCommand {
    chat_id: 123456,
    text: "Hello!".to_string(),
    parse_mode: Some(ParseMode::Markdown),
    reply_to_message_id: None,
    reply_markup: None,
};

publisher.publish(
    &subjects::agent::message_send("prod"),
    &command
).await?;
```

---

## 📝 Notes

1. **All messages are JSON-serialized** using serde
2. **Event IDs are UUIDs** for idempotency and tracking
3. **Timestamps are UTC** (ISO 8601 format)
4. **Session IDs are deterministic** based on chat/user context
5. **Streaming requires internal state** tracking by the bot

---

## 🔗 Related Files

- `rsworkspace/crates/telegram-nats/src/subjects.rs` - Subject definitions
- `rsworkspace/crates/telegram-types/src/commands.rs` - Command structures
- `rsworkspace/crates/telegram-types/src/events.rs` - Event structures
- `rsworkspace/crates/telegram-types/src/chat.rs` - Chat/User/Message types
