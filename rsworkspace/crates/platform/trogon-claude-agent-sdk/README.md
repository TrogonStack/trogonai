# Claude Agent SDK public API

This crate is an initial API draft for review. It exposes configuration and
client lifecycle signatures without a Claude Code runtime. `connect` and `close`
return `Error::NotImplemented`; they start no process and exchange no messages.

Configure the executable with `CliExecutable`, choose a working directory with
`cwd`, and optionally select a validated `ModelName`.

```rust
use std::path::PathBuf;
use trogon_claude_agent_sdk::{ClaudeAgentOptions, ClaudeSdkClient, ModelName};

# async fn example() -> Result<(), Box<dyn std::error::Error>> {
let options = ClaudeAgentOptions {
    cwd: Some(PathBuf::from(".")),
    model: Some(ModelName::new("sonnet")?),
    ..ClaudeAgentOptions::default()
};
let client = ClaudeSdkClient::connect(options).await?;
client.close().await?;
# Ok(())
# }
```

The example checks API composition; it does not run a session. Further options
and runtime behavior belong in separate review slices.
