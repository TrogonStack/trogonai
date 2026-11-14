#![cfg(feature = "test-support")]

//! The run-end ceiling comes from `CLAUDE_CODE_PRINT_BG_WAIT_CEILING_MS` in the environment the CLI is given.

use std::{error::Error as StdError, time::Duration};

use tokio::time::timeout;
use trogon_claude_agent_sdk::{
    ClaudeAgentOptions, ClaudeSdkClient, Error, Limits, Message, Prompt, Shutdown, Timeout, query,
};

type TestResult = Result<(), Box<dyn StdError>>;

const VARIABLE: &str = "CLAUDE_CODE_PRINT_BG_WAIT_CEILING_MS";

fn options(ceiling: &str) -> Result<ClaudeAgentOptions, Box<dyn StdError>> {
    let mut options = ClaudeAgentOptions {
        cli_path: Some(std::path::absolute(env!("CARGO_BIN_EXE_claude-sdk-fixture"))?),
        model: Some(String::from("fixture:stuck-running").into()),
        ..ClaudeAgentOptions::default()
    };
    options.limits.control_timeout = Some(Timeout::new(Duration::from_secs(2)));
    options.limits.shutdown = Shutdown::Strict(Timeout::new(Duration::from_millis(200)));
    // The fixture runs no crate code, so its coverage profile is discarded.
    options.env.insert("LLVM_PROFILE_FILE".into(), "/dev/null".into());
    options.env.insert(VARIABLE.into(), ceiling.into());
    Ok(options)
}

/// The fixture reports a result and never an idle state, so only the ceiling can end the run.
async fn run_until_it_ends(options: ClaudeAgentOptions) -> Result<Vec<String>, Box<dyn StdError>> {
    let mut response = query(Prompt::from("stuck"), options).await?;
    Ok(timeout(Duration::from_secs(5), async {
        let mut results = Vec::new();
        while let Some(message) = response.next_message().await? {
            if let Message::Result(result) = message {
                results.push(result.result.unwrap_or_default());
            }
        }
        Ok::<_, Error>(results)
    })
    .await??)
}

#[tokio::test]
async fn typescript_ends_the_run_at_the_ceiling_in_options_env() -> TestResult {
    assert_eq!(run_until_it_ends(options("300")?).await?, ["stuck"]);
    Ok(())
}

/// TypeScript also reads the spellings its parser accepts, and Python's `int()` does not.
#[tokio::test]
async fn typescript_reads_an_exponent_spelling_in_options_env() -> TestResult {
    assert_eq!(run_until_it_ends(options("3e2")?).await?, ["stuck"]);
    Ok(())
}

#[tokio::test]
async fn python_ends_the_run_at_the_ceiling_in_options_env() -> TestResult {
    let mut options = options("3_00")?;
    options.limits = Limits::python();
    options.limits.control_timeout = Some(Timeout::new(Duration::from_secs(2)));
    options.limits.shutdown = Shutdown::Strict(Timeout::new(Duration::from_millis(200)));
    assert_eq!(run_until_it_ends(options).await?, ["stuck"]);
    Ok(())
}

/// Zero is no limit: nothing ends a run that never reports an idle state.
#[tokio::test]
async fn zero_arms_no_ceiling() -> TestResult {
    let mut client = ClaudeSdkClient::connect(options("0")?).await?;
    client.send_prompt(Prompt::from("stuck")).await?;
    let still_open = timeout(Duration::from_millis(1500), async {
        let mut results = 0;
        while let Some(message) = client.next_message().await? {
            if matches!(message, Message::Result(_)) {
                results += 1;
            }
        }
        Ok::<_, Error>(results)
    })
    .await;
    assert!(still_open.is_err(), "the run ended without a ceiling: {still_open:?}");
    let _ = timeout(Duration::from_secs(15), client.disconnect()).await;
    Ok(())
}
