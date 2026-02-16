# Telegram Bot

A Telegram bot that acts as a bridge between Telegram and NATS, enabling AI agents to interact with users through Telegram.

## Features

- **Bidirectional Bridge**: Converts Telegram updates to NATS events and processes NATS commands to send messages back to Telegram
- **Message Types**: Supports text, photos, videos, and callback queries (button clicks)
- **Command Handling**: Parses and routes bot commands to appropriate NATS subjects
- **Access Control**: Configurable allow/deny lists for users and groups
- **Session Management**: Tracks user sessions using JetStream KV store
- **Streaming Support**: Progressive message updates for long-running AI responses
- **Inline Keyboards**: Support for interactive buttons in messages

## Architecture

```
┌─────────────┐         ┌──────────────┐         ┌─────────┐
│  Telegram   │◄───────►│ telegram-bot │◄───────►│  NATS   │
│             │         │              │         │         │
│ - Users     │         │ - Inbound    │         │ - Agent │
│ - Groups    │         │ - Outbound   │         │ - Events│
│ - Messages  │         │ - Sessions   │         │ - Cmds  │
└─────────────┘         └──────────────┘         └─────────┘
```

### Inbound Flow (Telegram → NATS)

1. User sends message to Telegram bot
2. Bot receives update from Telegram
3. Access control checks are performed
4. Message is converted to NATS event
5. Event is published to appropriate subject
6. Session state is updated in KV store

### Outbound Flow (NATS → Telegram)

1. Agent publishes command to NATS
2. Bot receives command from subscription
3. Command is converted to Telegram API call
4. Message/action is sent to Telegram user

## NATS Event Subjects

The bot publishes events to the following subjects:

### Inbound (Bot Events)

- `{prefix}.telegram.bot.message.text` - Text messages
- `{prefix}.telegram.bot.message.photo` - Photo messages
- `{prefix}.telegram.bot.message.video` - Video messages
- `{prefix}.telegram.bot.command.{command_name}` - Bot commands (e.g., `/start`)
- `{prefix}.telegram.bot.callback_query` - Button clicks

### Outbound (Agent Commands)

- `{prefix}.telegram.agent.message.send` - Send text message
- `{prefix}.telegram.agent.message.edit` - Edit existing message
- `{prefix}.telegram.agent.message.delete` - Delete message
- `{prefix}.telegram.agent.message.send_photo` - Send photo
- `{prefix}.telegram.agent.message.stream` - Streaming message update
- `{prefix}.telegram.agent.callback.answer` - Answer callback query
- `{prefix}.telegram.agent.chat.action` - Send chat action (typing indicator)

## Configuration

The bot can be configured via:

1. **Config file** (recommended): `config/telegram-bot.toml`
2. **Environment variables**: For containerized deployments
3. **CLI arguments**: For overriding specific values

### Config File

See `config/telegram-bot.example.toml` for a complete example.

```toml
[telegram]
bot_token = "YOUR_BOT_TOKEN"

[telegram.access]
admin_ids = [123456789]
allowed_user_ids = [123456789, 987654321]
allowed_group_ids = [-1001234567890]

[telegram.features]
inline_buttons = true
streaming = "partial"

[telegram.limits]
text_chunk_limit = 4096
media_max_mb = 50
history_limit = 100
rate_limit_messages_per_minute = 20

[nats]
servers = ["nats://localhost:4222"]
prefix = "prod"
```

### Environment Variables

```bash
# Required
TELEGRAM_BOT_TOKEN=your_bot_token_here

# Optional (override config file)
NATS_URL=nats://localhost:4222
TELEGRAM_PREFIX=prod
```

### CLI Arguments

```bash
# Configuration file
telegram-bot --config /path/to/config.toml

# Override specific values
telegram-bot \
  --bot-token "YOUR_TOKEN" \
  --nats-url "nats://localhost:4222" \
  --prefix "prod"
```

## Getting Started

### 1. Create a Bot

1. Message [@BotFather](https://t.me/BotFather) on Telegram
2. Send `/newbot` and follow the prompts
3. Save the bot token you receive

### 2. Get Your User ID

1. Message [@userinfobot](https://t.me/userinfobot)
2. Note your user ID

### 3. Configure the Bot

Create `config/telegram-bot.toml`:

```toml
[telegram]
bot_token = "YOUR_BOT_TOKEN_FROM_BOTFATHER"

[telegram.access]
admin_ids = [YOUR_USER_ID]
allowed_user_ids = [YOUR_USER_ID]

[nats]
servers = ["nats://localhost:4222"]
prefix = "dev"
```

### 4. Start NATS Server

```bash
# Using Docker
docker run -p 4222:4222 -p 8222:8222 nats:latest -js

# Or using nats-server binary
nats-server -js
```

### 5. Run the Bot

```bash
# From the workspace root
cargo run --bin telegram-bot

# Or with custom config
cargo run --bin telegram-bot -- --config /path/to/config.toml
```

## Development

### Building

```bash
# Development build
cargo build --package telegram-bot

# Release build
cargo build --package telegram-bot --release
```

### Running Tests

```bash
cargo test --package telegram-bot
```

### Logging

The bot uses `tracing` for logging. Control log level with `RUST_LOG`:

```bash
# Debug logging
RUST_LOG=telegram_bot=debug,telegram_nats=debug cargo run --bin telegram-bot

# Info logging (default)
RUST_LOG=info cargo run --bin telegram-bot
```

## Access Control

The bot supports three levels of access control:

1. **Admins**: Full access to all features
2. **Allowed Users**: Can use the bot in direct messages
3. **Allowed Groups**: Groups where the bot is active

Configure these in the `[telegram.access]` section of your config file.

## Session Management

Sessions are automatically created and managed for each chat. Session data includes:

- Session ID (derived from chat ID and type)
- Chat ID
- User ID
- Creation timestamp
- Last activity timestamp
- Message count

Sessions are stored in NATS JetStream KV store under the key pattern:
`{prefix}_telegram_sessions`

## Streaming Messages

The bot supports progressive message updates for long-running responses:

- **Disabled**: Send complete messages only
- **Partial**: Update message every N characters
- **Full**: Update message on every token/chunk

Configure in `[telegram.features]` section:

```toml
[telegram.features]
streaming = "partial"  # or "disabled" or "full"
```

## Error Handling

The bot implements robust error handling:

- Invalid messages are logged and ignored
- Failed API calls are logged but don't crash the bot
- Access denied messages are logged for monitoring
- Session errors fall back to graceful degradation

## License

See the main project LICENSE file.
