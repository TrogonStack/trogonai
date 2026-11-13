use std::sync::Arc;

use trogon_claude_agent_sdk::{
    ClaudeAgentOptions, HookEvent, HookFailureMode, HookOutput, HookRegistration, HookRequest, Message, Prompt, query,
};

#[cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    allow(
        debug_remnants,
        reason = "This example prints its defined result or connection status to the caller."
    )
)]
#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let options = ClaudeAgentOptions {
        hooks: vec![HookRegistration {
            event: HookEvent::PreToolUse,
            matcher: Some("Bash".into()),
            callbacks: vec![Arc::new(|_: HookRequest| async {
                Ok(HookOutput::block("Shell commands are disabled for this session."))
            })],
            timeout: None,
            on_failure: Some(HookFailureMode::Block),
        }],
        ..ClaudeAgentOptions::default()
    };
    let mut response = query(
        Prompt::from("Describe this directory without running shell commands."),
        options,
    )
    .await?;
    while let Some(message) = response.next_message().await? {
        if let Message::Result(result) = message {
            println!("{}", result.result.unwrap_or_default());
        }
    }
    Ok(())
}
