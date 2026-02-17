//! Example: Inline Mode Demo
//!
//! Demonstrates how inline queries work with the Telegram bot.
//!
//! ## Setup
//!
//! 1. Enable inline mode in BotFather:
//!    - Send `/mybots` to @BotFather
//!    - Select your bot
//!    - Choose "Bot Settings" → "Inline Mode" → "Turn on"
//!    - Set inline placeholder text (optional)
//!
//! 2. Start NATS server:
//!    ```
//!    nats-server -js
//!    ```
//!
//! 3. Run the bot:
//!    ```
//!    TELEGRAM_BOT_TOKEN=your_token cargo run --bin telegram-bot
//!    ```
//!
//! 4. Run the agent (echo mode):
//!    ```
//!    ENABLE_LLM=false cargo run --bin telegram-agent
//!    ```
//!
//! ## Usage
//!
//! In any Telegram chat (including private chats with other people):
//!
//! 1. Type `@your_bot_username` followed by your query
//! 2. You'll see inline results appear as you type
//! 3. Tap a result to send it to the chat
//!
//! ## Examples
//!
//! ```
//! @mybot hello world
//! ```
//!
//! In echo mode, you'll see three results:
//! - Echo: hello world
//! - HELLO WORLD (uppercase)
//! - hello world (lowercase)
//!
//! In LLM mode with Claude, you'll see an AI-generated response.
//!
//! ## Notes
//!
//! - Inline queries work in any chat, not just private messages with your bot
//! - Access control still applies (only allowed users can use inline mode)
//! - Results are cached for 5 minutes by default
//! - The bot must be in inline mode (enable via BotFather)

fn main() {
    println!("This is a documentation-only example.");
    println!("Follow the setup instructions in the source code.");
    println!("\nFile: examples/inline_mode_demo.rs");
}
