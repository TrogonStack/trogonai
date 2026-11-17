#![cfg(all(unix, feature = "test-support"))]

//! TypeScript has no bundled executable lookup; Python's `_find_cli` takes `<package>/_bundled/claude` before
//! `PATH`. This binary holds one test because it points the process `PATH` at the fixture.

use std::{error::Error, path::PathBuf, time::Duration};

use tokio::time::timeout;
use trogon_claude_agent_sdk::{ClaudeAgentOptions, ClaudeSdkClient, Error as SdkError, Message, Prompt, Timeout};

fn options(bundled_cli_root: Option<PathBuf>) -> ClaudeAgentOptions {
    let mut options = ClaudeAgentOptions {
        model: Some("fixture:normal".into()),
        bundled_cli_root,
        ..ClaudeAgentOptions::default()
    };
    options.limits.control_timeout = Some(Timeout::new(Duration::from_secs(2)));
    options.limits.shutdown = trogon_claude_agent_sdk::Shutdown::Strict(Timeout::new(Duration::from_millis(200)));
    // The fixture runs no crate code, so its coverage profile is discarded.
    options.env.insert("LLVM_PROFILE_FILE".into(), "/dev/null".into());
    options
}

async fn read_result(client: &mut ClaudeSdkClient) -> Result<String, Box<dyn Error>> {
    timeout(Duration::from_secs(5), async {
        while let Some(message) = client.next_message().await? {
            if let Message::Result(result) = message {
                return Ok(result.result.clone().unwrap_or_default());
            }
        }
        Err(std::io::Error::other("fixture ended without a result").into())
    })
    .await?
}

/// Reading the result and the disconnect after it both failed; each error is kept for the report.
#[derive(Debug)]
struct ReadAndDisconnectFailed {
    read: Box<dyn Error>,
    disconnect: SdkError,
}

impl std::fmt::Display for ReadAndDisconnectFailed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "reading the result failed: {}; disconnecting failed: {}",
            self.read, self.disconnect
        )
    }
}

impl Error for ReadAndDisconnectFailed {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.read.as_ref())
    }
}

#[cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    allow(
        std_env_access,
        reason = "Discovery reads the process PATH, so the test points it at a directory that holds the fixture."
    )
)]
#[tokio::test]
async fn only_a_named_bundled_root_is_searched_before_path() -> Result<(), Box<dyn Error>> {
    let root = tempfile::tempdir()?;
    let bin = root.path().join("bin");
    std::fs::create_dir(&bin)?;
    std::os::unix::fs::symlink(
        std::path::absolute(env!("CARGO_BIN_EXE_claude-sdk-fixture"))?,
        bin.join("claude"),
    )?;
    let package = root.path().join("package");
    std::fs::create_dir_all(package.join("_bundled"))?;
    // A plain file is enough for Python's lookup, and spawning it fails, which tells the two lookups apart.
    std::fs::write(package.join("_bundled/claude"), "not an executable")?;
    // SAFETY: this binary holds one test and nothing else runs yet, so no other thread reads the environment.
    unsafe { std::env::set_var("PATH", &bin) };

    let mut client = ClaudeSdkClient::connect(options(None)).await?;
    client.send_prompt(Prompt::from("from path")).await?;
    let observed = read_result(&mut client).await;
    let cleanup = client.disconnect().await;
    let text = match (observed, cleanup) {
        (Err(read), Err(disconnect)) => return Err(ReadAndDisconnectFailed { read, disconnect }.into()),
        (observed, cleanup) => {
            let text = observed?;
            cleanup?;
            text
        }
    };
    assert_eq!(text, "from path");

    let error = ClaudeSdkClient::connect(options(Some(package))).await.err();
    assert!(matches!(error, Some(SdkError::Spawn(_))), "{error:?}");
    Ok(())
}
