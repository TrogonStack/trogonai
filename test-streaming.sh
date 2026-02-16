#!/bin/bash
# Script para probar el sistema de streaming completo

set -e

echo "🧪 TrogonAI Streaming Test Suite"
echo "================================="
echo ""

# Check if NATS is running
if ! pgrep -f nats-server > /dev/null; then
    echo "❌ NATS server is not running"
    echo "   Start it with: nats-server"
    exit 1
fi
echo "✅ NATS server is running"

# Check if bot token is set
if [ -z "$TELEGRAM_BOT_TOKEN" ]; then
    echo ""
    echo "⚠️  TELEGRAM_BOT_TOKEN not set"
    echo ""
    echo "To get a bot token:"
    echo "  1. Open Telegram and search for @BotFather"
    echo "  2. Send: /newbot"
    echo "  3. Follow the prompts"
    echo "  4. Copy the token and run:"
    echo ""
    echo "     export TELEGRAM_BOT_TOKEN='your-token-here'"
    echo "     ./test-streaming.sh"
    echo ""
    exit 1
fi
echo "✅ Bot token configured"

# Build everything
echo ""
echo "🔨 Building telegram-bot..."
cargo build --package telegram-bot --quiet
echo "✅ Build complete"

echo ""
echo "🔨 Building test-streaming example..."
cargo build --package telegram-bot --example test_streaming --quiet
echo "✅ Test example built"

# Run unit tests
echo ""
echo "🧪 Running unit tests..."
cargo test --package telegram-bot --test streaming_test --quiet
echo "✅ All unit tests passed"

echo ""
echo "================================================"
echo "🚀 Starting Telegram Bot"
echo "================================================"
echo ""
echo "The bot will:"
echo "  ✓ Connect to NATS"
echo "  ✓ Subscribe to streaming messages"
echo "  ✓ Apply rate limiting (1 edit/second)"
echo "  ✓ Retry failed operations (up to 3 times)"
echo "  ✓ Track message IDs for progressive edits"
echo ""
echo "Press Ctrl+C to stop the bot"
echo ""
echo "In another terminal, run:"
echo "  cargo run --package telegram-bot --example test_streaming"
echo ""
echo "Or send messages to your bot in Telegram!"
echo ""

# Run the bot
export NATS_URL="localhost:4222"
export TELEGRAM_PREFIX="test"
export RUST_LOG="telegram_bot=debug,telegram_nats=debug,info"

cargo run --package telegram-bot
