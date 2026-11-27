use super::*;
use std::time::Duration;

fn long_running() -> tokio::process::Command {
    #[cfg(windows)]
    {
        let mut command = tokio::process::Command::new("cmd");
        command.args(["/c", "ping -n 60 127.0.0.1 >nul"]);
        command
    }
    #[cfg(not(windows))]
    {
        let mut command = tokio::process::Command::new("sleep");
        command.arg("60");
        command
    }
}

fn quick() -> tokio::process::Command {
    #[cfg(windows)]
    {
        let mut command = tokio::process::Command::new("cmd");
        command.args(["/c", "exit 3"]);
        command
    }
    #[cfg(not(windows))]
    {
        let mut command = tokio::process::Command::new("sh");
        command.args(["-c", "exit 3"]);
        command
    }
}

#[tokio::test]
async fn an_interrupt_kills_the_command_and_reads_as_a_clean_exit() -> Result<(), Box<dyn std::error::Error>> {
    let mut child = long_running().kill_on_drop(true).spawn()?;
    let outcome = tokio::time::timeout(
        Duration::from_secs(20),
        wait_forwarding(&mut child, async {
            tokio::time::sleep(Duration::from_millis(300)).await
        }),
    )
    .await
    .map_err(|_| "the wait did not end")??;
    assert!(matches!(outcome, Outcome::Interrupted));
    assert_eq!(outcome.exit_code(), 0);
    assert!(child.try_wait()?.is_some());
    Ok(())
}

#[tokio::test]
async fn a_command_that_ends_by_itself_keeps_its_exit_code() -> Result<(), Box<dyn std::error::Error>> {
    let mut child = quick().spawn()?;
    let outcome = wait_forwarding(&mut child, std::future::pending()).await?;
    assert_eq!(outcome.exit_code(), 3);
    Ok(())
}
