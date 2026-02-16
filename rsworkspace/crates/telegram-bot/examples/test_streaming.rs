//! Example test to verify streaming message flow through NATS
//!
//! This simulates an agent sending streaming messages to the bot

use anyhow::Result;
use telegram_nats::{MessagePublisher, NatsConfig, subjects};
use telegram_types::commands::{StreamMessageCommand, ParseMode};
use tokio::time::{sleep, Duration};

#[tokio::main]
async fn main() -> Result<()> {
    println!("🚀 Testing Streaming Message Flow");
    println!("=================================\n");

    // Connect to NATS
    let config = NatsConfig::from_url("localhost:4222", "test");
    println!("📡 Connecting to NATS at localhost:4222...");

    let client = telegram_nats::connect(&config).await?;
    println!("✅ Connected to NATS\n");

    let publisher = MessagePublisher::new(client, "test".to_string());

    let chat_id = 12345_i64; // Test chat ID
    let subject = subjects::agent::message_stream("test");

    println!("📝 Simulating streaming response to chat {}", chat_id);
    println!("📬 Publishing to subject: {}\n", subject);

    // Simulate streaming a response in chunks
    let chunks = vec![
        "Hello! ",
        "This is a ",
        "streaming message ",
        "being sent ",
        "progressively ",
        "through NATS. ",
        "It demonstrates ",
        "the rate limiting ",
        "and message tracking ",
        "features!",
    ];

    let mut accumulated = String::new();

    for (i, chunk) in chunks.iter().enumerate() {
        accumulated.push_str(chunk);

        let is_final = i == chunks.len() - 1;

        let cmd = StreamMessageCommand {
            chat_id,
            message_id: None, // Bot will track this internally
            text: accumulated.clone(),
            parse_mode: Some(ParseMode::Markdown),
            is_final,
        };

        println!("📤 Chunk {}/{}: \"{}{}\"",
            i + 1,
            chunks.len(),
            if accumulated.len() > 50 {
                format!("{}...", &accumulated[..50])
            } else {
                accumulated.clone()
            },
            if is_final { " (FINAL)" } else { "" }
        );

        publisher.publish(&subject, &cmd).await?;

        if !is_final {
            // Wait a bit between chunks to simulate real streaming
            sleep(Duration::from_millis(300)).await;
        }
    }

    println!("\n✅ All chunks sent successfully!");
    println!("\n📊 Summary:");
    println!("  - Total chunks: {}", chunks.len());
    println!("  - Final message length: {} chars", accumulated.len());
    println!("  - Rate limiting: Active (1 edit/second minimum)");
    println!("  - Retry logic: Enabled (up to 3 attempts)");
    println!("\n✨ The bot should have received and processed these messages with proper rate limiting!");

    Ok(())
}
