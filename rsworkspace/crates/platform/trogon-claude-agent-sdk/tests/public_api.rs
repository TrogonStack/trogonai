use std::path::PathBuf;

use trogon_claude_agent_sdk::ClaudeAgentOptions;

#[cfg(all(unix, feature = "test-support"))]
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
#[cfg(all(unix, feature = "test-support"))]
use trogon_claude_agent_sdk::{
    ClaudeSdkClient,
    adapters::{NativeFileSystem, RuntimeAdapters},
    test_support::FixedEnvironment,
};

#[cfg(all(unix, feature = "test-support"))]
#[path = "support/scripted_process.rs"]
mod scripted_process;

#[cfg(all(unix, feature = "test-support"))]
use scripted_process::{ScriptedProcessEvent, ScriptedProcessSpawner};

/// A usable owner requires a real initialization reply, and disconnect closes the child's input.
#[cfg(all(unix, feature = "test-support"))]
#[tokio::test]
async fn connect_waits_for_initialization_and_disconnect_closes_input() -> Result<(), Box<dyn std::error::Error>> {
    let (process, harness) = ScriptedProcessSpawner::new();
    let observer = process.observer();
    let connecting = tokio::spawn(ClaudeSdkClient::connect_with_adapters(
        ClaudeAgentOptions::default(),
        RuntimeAdapters {
            environment: FixedEnvironment {
                directory: PathBuf::from("unused-memory-directory"),
                variables: Default::default(),
            },
            process,
            filesystem: NativeFileSystem,
        },
    ));
    let mut input = BufReader::new(harness.stdin).lines();
    let request: serde_json::Value = serde_json::from_str(
        &input
            .next_line()
            .await?
            .ok_or_else(|| std::io::Error::other("missing initialize"))?,
    )?;
    assert_eq!(request["request"]["subtype"], "initialize");
    assert!(!connecting.is_finished());
    let server = tokio::spawn(async move {
        let mut stdout = harness.stdout;
        let response = format!(
            "{{\"type\":\"control_response\",\"response\":{{\"subtype\":\"success\",\"request_id\":{},\"response\":{{\"claude_code_version\":\"fixture\"}}}}}}\n",
            request["request_id"],
        );
        stdout.write_all(response.as_bytes()).await?;
        assert!(input.next_line().await?.is_none());
        drop(stdout);
        drop(harness.stderr);
        harness.observer.complete(0);
        Ok::<_, std::io::Error>(())
    });
    let client = connecting.await??;
    client.disconnect().await?;
    server.await??;
    assert!(observer.events()?.contains(&ScriptedProcessEvent::StdinClosed));
    Ok(())
}

/// Preserve unrestricted upstream configuration values for the runtime to interpret.
#[test]
fn configuration_preserves_upstream_optional_values() {
    let defaults = ClaudeAgentOptions::default();
    assert_eq!(defaults.cli_path, None);
    assert_eq!(defaults.cwd, None);
    assert_eq!(defaults.model, None);
    assert_eq!(defaults.permission_mode, None);

    for (model, cli_path) in [("", ""), ("model\nname", "tools/claude")] {
        let options = ClaudeAgentOptions {
            cli_path: Some(PathBuf::from(cli_path)),
            cwd: Some(PathBuf::from("workspace")),
            model: Some(model.into()),
            ..ClaudeAgentOptions::default()
        };
        assert_eq!(options.cli_path, Some(PathBuf::from(cli_path)));
        assert_eq!(options.cwd, Some(PathBuf::from("workspace")));
        assert_eq!(
            options.model.as_ref().map(trogon_claude_agent_sdk::ModelName::as_str),
            Some(model)
        );
    }
}
