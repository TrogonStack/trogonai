use std::path::Path;

use super::command;
use crate::{ClaudeAgentOptions, environment_plan::Inherited, transport::Transport};

fn current_uid() -> u32 {
    // SAFETY: `getuid` takes no arguments and cannot fail.
    unsafe { libc::getuid() }
}

async fn run(options: &ClaudeAgentOptions) -> Result<std::process::ExitStatus, std::io::Error> {
    let mut command = command(options, Path::new("true"), &Inherited::new(), None).map_err(std::io::Error::other)?;
    command.spawn()?.wait().await
}

#[tokio::test]
async fn running_as_the_current_uid_needs_no_privilege() -> Result<(), std::io::Error> {
    let options = ClaudeAgentOptions {
        run_as_uid: Some(current_uid()),
        ..ClaudeAgentOptions::default()
    };
    assert!(run(&options).await?.success());
    Ok(())
}

#[tokio::test]
async fn a_named_user_is_resolved_and_the_uid_switch_is_applied_in_the_child() -> Result<(), std::io::Error> {
    let options = ClaudeAgentOptions {
        user: Some("root".into()),
        run_as_uid: Some(current_uid()),
        ..ClaudeAgentOptions::default()
    };
    let outcome = run(&options).await;
    if current_uid() == 0 {
        assert!(outcome?.success());
    } else {
        assert_eq!(
            outcome.err().map(|e| e.kind()),
            Some(std::io::ErrorKind::PermissionDenied)
        );
    }
    Ok(())
}

#[tokio::test]
async fn the_unchanged_identity_uid_is_refused_before_spawning() {
    let options = ClaudeAgentOptions {
        run_as_uid: Some(u32::MAX),
        ..ClaudeAgentOptions::default()
    };
    let error = command(&options, Path::new("true"), &Inherited::new(), None).err();
    assert!(matches!(error, Some(crate::Error::Io(ref io)) if io.kind() == std::io::ErrorKind::InvalidInput));
}

#[tokio::test]
async fn an_unknown_user_fails_before_spawning() {
    let options = ClaudeAgentOptions {
        user: Some("trogon-no-such-user-x".into()),
        ..ClaudeAgentOptions::default()
    };
    let error = command(&options, Path::new("true"), &Inherited::new(), None).err();
    assert!(matches!(error, Some(crate::Error::Io(ref io)) if io.kind() == std::io::ErrorKind::NotFound));
}

#[tokio::test]
async fn killing_a_process_that_already_exited_succeeds_and_marks_the_stop_as_forced()
-> Result<(), Box<dyn std::error::Error>> {
    let options = ClaudeAgentOptions::default();
    let mut transport = super::ProcessTransport::spawn(super::ProcessSpawn {
        options: &options,
        cli_path: "true".into(),
        inherited: &Inherited::new(),
        process: &crate::adapters::TokioProcessSpawner,
        config_dir: None,
        derived_debug_file: None,
        secure_storage_config_dir: None,
        closed: super::Closed::default(),
    })?;
    let child = transport.child.as_mut().ok_or("the child was handed off")?;
    assert!(child.wait().await?.success());
    transport.kill().await?;
    assert!(transport.forced);
    Ok(())
}
