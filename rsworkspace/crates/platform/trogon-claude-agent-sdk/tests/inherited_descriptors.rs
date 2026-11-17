#![cfg(all(unix, feature = "test-support"))]

//! Python's `Popen(close_fds=True)` passes the CLI only its stdio on every platform; TypeScript's libuv spawn does on
//! Apple platforms and keeps inheritable descriptors elsewhere. This binary holds one test because it opens an
//! inheritable descriptor on purpose, which a spawn in a concurrent test would also hand its child.

use std::{
    error::Error,
    os::fd::{AsRawFd, OwnedFd},
    time::Duration,
};

use tokio::{io::AsyncReadExt, net::unix::pipe, time::timeout};
use trogon_claude_agent_sdk::{ArgvStyle, ClaudeAgentOptions, ClaudeSdkClient, Timeout};

/// How long the read end waits for EOF once nothing but the child can hold the write end.
const EOF_WAIT: Duration = Duration::from_secs(1);

fn options(argv_style: ArgvStyle) -> Result<ClaudeAgentOptions, Box<dyn Error>> {
    let mut options = ClaudeAgentOptions {
        cli_path: Some(std::path::absolute(env!("CARGO_BIN_EXE_claude-sdk-fixture"))?),
        model: Some("fixture:normal".into()),
        argv_style,
        ..ClaudeAgentOptions::default()
    };
    options.limits.control_timeout = Some(Timeout::new(Duration::from_secs(2)));
    options.limits.shutdown = trogon_claude_agent_sdk::Shutdown::Strict(Timeout::new(Duration::from_millis(200)));
    // The fixture runs no crate code, so its coverage profile is discarded.
    options.env.insert("LLVM_PROFILE_FILE".into(), "/dev/null".into());
    Ok(options)
}

/// Starts the fixture while the parent holds the inheritable write end of a pipe, drops that end, and answers what
/// a read of the other end saw while the fixture ran (`None` when it was still waiting) and after it stopped.
async fn reads_around_the_child(argv_style: ArgvStyle) -> Result<(Option<usize>, usize), Box<dyn Error>> {
    let (reader, writer) = std::io::pipe()?;
    let writer = OwnedFd::from(writer);
    // SAFETY: `F_SETFD` only changes the flags of a descriptor this function owns. The standard library has no
    // safe way to make a descriptor inheritable.
    let cleared = unsafe { libc::fcntl(writer.as_raw_fd(), libc::F_SETFD, 0) };
    assert_eq!(cleared, 0);
    let client = ClaudeSdkClient::connect(options(argv_style)?).await?;
    drop(writer);
    let mut receiver = pipe::Receiver::from_owned_fd(OwnedFd::from(reader))?;
    let mut byte = [0u8; 1];
    let while_running = timeout(EOF_WAIT, receiver.read(&mut byte)).await.ok().transpose()?;
    client.disconnect().await?;
    let after_stopping = timeout(EOF_WAIT, receiver.read(&mut byte)).await??;
    Ok((while_running, after_stopping))
}

/// The SDK's own spawn leaves the child no descriptor above stderr, even one the parent left inheritable, under the
/// Python dialect everywhere and under the TypeScript dialect on Apple platforms. Elsewhere the TypeScript dialect
/// lets the child keep it until it exits, as Node does.
#[tokio::test]
async fn the_child_keeps_inheritable_descriptors_only_where_the_dialect_upstream_does() -> Result<(), Box<dyn Error>> {
    assert_eq!(reads_around_the_child(ArgvStyle::Python).await?, (Some(0), 0));
    let typescript = cfg!(target_vendor = "apple").then_some(0);
    assert_eq!(reads_around_the_child(ArgvStyle::TypeScript).await?, (typescript, 0));
    Ok(())
}
