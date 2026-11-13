use std::{io, path::PathBuf, process::Stdio, time::Duration};

use tokio::{
    process::{Child, Command},
    time::timeout,
};

use super::{Closed, ProcessSpawn, ProcessTransport};
use crate::{
    ArgvStyle, ClaudeAgentOptions, Error, Transport,
    adapters::{SpawnProcess, TokioProcessSpawner},
    environment_plan::Inherited,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Runs a shell script in place of the CLI, with every pipe the transport takes.
struct Script(&'static str);

impl SpawnProcess for Script {
    type Error = io::Error;

    fn spawn(&self, _: &mut Command) -> io::Result<Child> {
        let mut shell = Command::new("sh");
        shell
            .args(["-c", self.0])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        TokioProcessSpawner.spawn(&mut shell)
    }
}

fn spawn(script: &'static str, argv_style: ArgvStyle) -> Result<ProcessTransport, Error> {
    let options = ClaudeAgentOptions {
        argv_style,
        ..ClaudeAgentOptions::default()
    };
    ProcessTransport::spawn(ProcessSpawn {
        options: &options,
        cli_path: PathBuf::from("claude"),
        inherited: &Inherited::new(),
        process: &Script(script),
        config_dir: None,
        derived_debug_file: None,
        secure_storage_config_dir: None,
        closed: Closed::default(),
    })
}

#[tokio::test]
async fn a_write_to_an_exited_process_with_stdin_still_open_reports_each_upstream_state() -> TestResult {
    for (style, expected) in [
        (ArgvStyle::Python, "Cannot write to terminated process (exit code: 3)"),
        (ArgvStyle::TypeScript, "ProcessTransport is not ready for writing"),
    ] {
        let mut transport = spawn("exit 3", style)?;
        timeout(
            Duration::from_secs(10),
            transport.child.as_mut().ok_or("missing child")?.wait(),
        )
        .await??;
        let error = transport
            .write("{}\n")
            .await
            .err()
            .ok_or("write after exit succeeded")?;
        assert_eq!(error.to_string(), expected, "{style:?}");
    }
    Ok(())
}

#[tokio::test]
async fn kill_stops_a_running_process() -> TestResult {
    let mut transport = spawn("exec sleep 30", ArgvStyle::TypeScript)?;
    transport.kill().await?;
    let status = timeout(
        Duration::from_secs(10),
        transport.child.as_mut().ok_or("missing child")?.wait(),
    )
    .await??;
    assert!(!status.success());
    assert!(transport.forced);
    Ok(())
}
