//! A lease on an image whose own command exits at once (plain `alpine` runs
//! `/bin/sh`, which quits without a TTY) must still yield a usable sandbox.
//!
//! ```text
//! cargo test -p trogon-sandbox-docker-worker exited_image_command
//! ```

mod docker_support;

use docker_support::{TestResult, build_provider, fresh_lease_id, lease_request};
use trogon_sandbox_contract::BootMode;
use trogon_sandbox_contract::{CommandEnv, StreamedOutput};
use trogon_sandbox_protocol::Provider;

#[tokio::test(flavor = "multi_thread")]
async fn a_lease_on_an_image_whose_command_exits_stays_usable() -> TestResult {
    let provider = build_provider("exited-image-command").await?;
    let lease_id = fresh_lease_id();
    provider
        .lease(&lease_request("exited-image-command-test", BootMode::Bare)?, &lease_id)
        .await?;

    let output = provider
        .run_command_for_lease(
            &lease_id,
            vec!["echo".to_string(), "alive".to_string()],
            None,
            &CommandEnv::default(),
            30,
            None,
            StreamedOutput::Disabled,
            usize::MAX,
        )
        .await?;
    assert_eq!(output.exit_code, 0);
    assert!(String::from_utf8_lossy(&output.stdout).contains("alive"));
    provider.cancel(&lease_id).await?;
    Ok(())
}
