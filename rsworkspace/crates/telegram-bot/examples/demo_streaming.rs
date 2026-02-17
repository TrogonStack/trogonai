//! Demo of streaming functionality without requiring a real Telegram bot
//!
//! This simulates both the agent sending chunks and the bot processing them

use std::time::Duration;
use tokio::time::{sleep, Instant};

struct MockStreamingProcessor {
    message_id: Option<i32>,
    last_edit: Instant,
    edit_count: u32,
}

impl MockStreamingProcessor {
    fn new() -> Self {
        Self {
            message_id: None,
            last_edit: Instant::now(),
            edit_count: 0,
        }
    }

    async fn process_chunk(&mut self, text: &str, is_final: bool) {
        const MIN_EDIT_INTERVAL: Duration = Duration::from_millis(1000);

        if let Some(msg_id) = self.message_id {
            // Editing existing message
            let time_since_last = self.last_edit.elapsed();

            if time_since_last < MIN_EDIT_INTERVAL {
                let wait_time = MIN_EDIT_INTERVAL - time_since_last;
                println!("  ⏱️  Rate limiting: waiting {:?} before edit", wait_time);
                sleep(wait_time).await;
            }

            self.edit_count += 1;
            println!(
                "  ✏️  Edit #{}: Message {} → \"{}\"{}",
                self.edit_count,
                msg_id,
                if text.len() > 60 {
                    format!("{}...", &text[..60])
                } else {
                    text.to_string()
                },
                if is_final { " (FINAL)" } else { "" }
            );

            self.last_edit = Instant::now();

            if is_final {
                println!("  🏁 Streaming complete! Cleaning up tracking...");
                self.message_id = None;
                self.edit_count = 0;
            }
        } else {
            // Creating new message
            self.message_id = Some(42_i32); // Mock message ID
            self.last_edit = Instant::now();

            println!(
                "  📤 New message {}: \"{}\"",
                self.message_id.unwrap(),
                if text.len() > 60 {
                    format!("{}...", &text[..60])
                } else {
                    text.to_string()
                }
            );

            if is_final {
                println!("  ℹ️  Message was final on first send (no more updates)");
            }
        }
    }
}

#[tokio::main]
async fn main() {
    println!("🎬 TrogonAI Streaming Demo");
    println!("==========================\n");

    let mut processor = MockStreamingProcessor::new();

    // Simulate LLM generating a response in chunks
    let response_chunks = vec![
        ("Hello! ", false),
        ("I'm an AI assistant ", false),
        ("powered by TrogonAI. ", false),
        ("I can stream my responses ", false),
        ("progressively, ", false),
        ("just like ChatGPT! ", false),
        ("This demonstrates:\n\n", false),
        ("✓ Rate limiting (1 edit/second)\n", false),
        ("✓ Message tracking by session\n", false),
        ("✓ Progressive edits\n", false),
        ("✓ Proper cleanup with is_final flag\n\n", false),
        ("All working perfectly! 🚀", true),
    ];

    println!("📝 Simulating LLM streaming response...\n");

    let mut accumulated = String::new();
    let start_time = Instant::now();

    for (i, (chunk, is_final)) in response_chunks.iter().enumerate() {
        accumulated.push_str(chunk);

        println!("📊 Chunk {}/{} received", i + 1, response_chunks.len());

        processor.process_chunk(&accumulated, *is_final).await;

        if !is_final {
            // Simulate time between LLM chunks
            sleep(Duration::from_millis(200)).await;
        }

        println!();
    }

    let total_time = start_time.elapsed();

    println!("✅ Demo Complete!\n");
    println!("📊 Statistics:");
    println!("  • Total chunks processed: {}", response_chunks.len());
    println!("  • Total edits performed: {}", processor.edit_count);
    println!("  • Time elapsed: {:.2}s", total_time.as_secs_f64());
    println!("  • Final message length: {} chars", accumulated.len());
    println!("\n🎯 Features Demonstrated:");
    println!("  ✓ Initial message creation");
    println!("  ✓ Progressive message editing");
    println!("  ✓ Rate limiting (1 edit/second minimum)");
    println!("  ✓ Message tracking");
    println!("  ✓ Proper cleanup on final chunk");
    println!("\n💡 In production:");
    println!("  • This would send actual Telegram API calls");
    println!("  • Messages would appear in Telegram chat");
    println!("  • Rate limiting prevents API throttling");
    println!("  • Retry logic handles transient failures");
}
