# Telegram Agent

AI agent that processes Telegram messages and generates responses.

## Overview

The Telegram Agent listens to Telegram events from NATS and processes them to generate intelligent responses. It's designed to work in tandem with the `telegram-bot` which handles the Telegram Bot API communication.

## Architecture

```
┌─────────────┐         ┌──────────────┐         ┌─────────────┐
│  Telegram   │◄───────►│ telegram-bot │◄───────►│    NATS     │
│             │         │              │         │             │
└─────────────┘         └──────────────┘         └─────────────┘
                                                         ▲
                                                         │
                                                         ▼
                                                 ┌──────────────┐
                                                 │telegram-agent│
                                                 │              │
                                                 │ - Processes  │
                                                 │ - Responds   │
                                                 │ - (Future:   │
                                                 │   LLM)       │
                                                 └──────────────┘
```

## Components

### Agent (`agent.rs`)
- Subscribes to NATS events
- Routes messages to appropriate processors
- Runs multiple handlers concurrently

### Processor (`processor.rs`)
- Business logic for message processing
- Currently implements echo responses
- **TODO**: LLM integration

## Message Handling

### Text Messages
1. Receives `MessageTextEvent` from NATS
2. Sends typing indicator
3. Processes message (currently echoes back)
4. Publishes `SendMessageCommand` to NATS

### Commands
Supports these commands:
- `/start` - Welcome message
- `/help` - Show available commands
- `/status` - Bot status

### Photo Messages
- Receives photo events
- Acknowledges receipt
- Returns confirmation message

### Callback Queries
- Handles button clicks
- Answers callback query
- Sends confirmation message

## Configuration

Via environment variables or CLI args:

```bash
# NATS connection
NATS_URL=nats://localhost:4222

# Environment prefix
TELEGRAM_PREFIX=prod

# Agent identification
AGENT_NAME=telegram-agent
```

## Running

```bash
# Development
cargo run --bin telegram-agent

# With custom config
cargo run --bin telegram-agent -- \
  --nats-url nats://localhost:4222 \
  --prefix dev \
  --agent-name my-agent

# Release build
cargo build --release --bin telegram-agent
./target/release/telegram-agent
```

## Message Flow

### Inbound (User → Agent)

```
User sends message to Telegram
    ↓
telegram-bot receives via Bot API
    ↓
Bot publishes event to NATS
    ↓
telegram-agent receives event
    ↓
Agent processes message
    ↓
Agent publishes command to NATS
    ↓
Bot sends response via Bot API
    ↓
User receives response
```

## NATS Subjects

### Subscribed (Input)
- `{prefix}.telegram.bot.message.text` - Text messages
- `{prefix}.telegram.bot.message.photo` - Photo messages
- `{prefix}.telegram.bot.command.*` - Commands (wildcard)
- `{prefix}.telegram.bot.callback_query` - Button clicks

### Published (Output)
- `{prefix}.telegram.agent.message.send` - Send message
- `{prefix}.telegram.agent.callback.answer` - Answer callback
- `{prefix}.telegram.agent.chat.action` - Chat actions (typing)

## Future Enhancements

### Phase 2: LLM Integration
- [ ] Add Claude/OpenAI API integration
- [ ] Implement prompt management
- [ ] Handle context windows
- [ ] Token counting and management

### Phase 3: Advanced Features
- [ ] Multi-turn conversation tracking
- [ ] Conversation memory/context
- [ ] File download and processing
- [ ] Media generation
- [ ] Streaming responses
- [ ] Rate limiting per user
- [ ] Response caching

### Phase 4: Intelligence
- [ ] Intent recognition
- [ ] Entity extraction
- [ ] Sentiment analysis
- [ ] Context-aware responses
- [ ] Personality customization

## Development

### Building

```bash
cargo build --package telegram-agent
```

### Testing

```bash
# Run tests
cargo test --package telegram-agent

# With logging
RUST_LOG=debug cargo test --package telegram-agent
```

### Adding New Message Handlers

1. Add handler function to `agent.rs`:
```rust
async fn handle_new_type(&self) -> Result<()> {
    // Subscribe to NATS subject
    // Process messages
    // Call processor
}
```

2. Add processing logic to `processor.rs`:
```rust
pub async fn process_new_type(&self, event: &Event, publisher: &MessagePublisher) -> Result<()> {
    // Business logic here
}
```

3. Add handler to `run()` in `agent.rs`:
```rust
tokio::try_join!(
    // ... existing handlers,
    self.handle_new_type()
)?;
```

## Dependencies

- `telegram-types` - Shared type definitions
- `telegram-nats` - NATS client wrapper
- `tokio` - Async runtime
- `async-nats` - NATS client
- `tracing` - Structured logging
- `clap` - CLI parsing

## License

See main project LICENSE file.
