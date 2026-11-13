use trogon_claude_agent_sdk::{ClaudeAgentOptions, ContentBlock, Message, Prompt, query};

#[cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    allow(
        debug_remnants,
        reason = "This example prints its defined result or connection status to the caller."
    )
)]
#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut query = query(
        Prompt::from("Explain this project's purpose without changing files."),
        ClaudeAgentOptions::default(),
    )
    .await?;
    while let Some(message) = query.next_message().await? {
        if let Message::Assistant(message) = message {
            for block in message.content {
                if let ContentBlock::Text { text } = block {
                    println!("{text}");
                }
            }
        }
    }
    Ok(())
}
