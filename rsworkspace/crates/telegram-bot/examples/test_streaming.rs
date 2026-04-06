use anyhow::Result;
use telegram_nats::{subjects, MessagePublisher};
use telegram_types::commands::{ParseMode, StreamMessageCommand};
use tokio::time::{Duration, sleep};

#[tokio::main]
async fn main() -> Result<()> {
    println!("Testing Streaming Message Flow");

    let client = async_nats::connect("localhost:4222").await?;
    println!("Connected to NATS");

    let publisher = MessagePublisher::new(client, "test".to_string());

    let chat_id = 12345_i64;
    let subject = subjects::agent::message_stream("test");

    println!("Simulating streaming response to chat {}", chat_id);
    println!("Publishing to subject: {}", subject);

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
            message_id: None,
            text: accumulated.clone(),
            parse_mode: Some(ParseMode::Markdown),
            is_final,
            session_id: None,
            message_thread_id: None,
        };

        println!(
            "Chunk {}/{}: \"{}{}\"",
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
            sleep(Duration::from_millis(300)).await;
        }
    }

    println!("All chunks sent successfully!");
    println!("Total chunks: {}", chunks.len());
    println!("Final message length: {} chars", accumulated.len());

    Ok(())
}
