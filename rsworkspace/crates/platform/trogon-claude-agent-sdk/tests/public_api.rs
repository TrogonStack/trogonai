use std::{
    future::Future,
    path::PathBuf,
    pin::pin,
    task::{Context, Poll, Waker},
};

use trogon_claude_agent_sdk::{ClaudeAgentOptions, ClaudeSdkClient, Error, ModelName};

/// The API draft must not pretend that a connection was established.
#[test]
fn connect_reports_unimplemented_runtime() {
    let mut connection = pin!(ClaudeSdkClient::connect(ClaudeAgentOptions::default()));
    let mut context = Context::from_waker(Waker::noop());
    assert!(matches!(
        connection.as_mut().poll(&mut context),
        Poll::Ready(Err(Error::NotImplemented))
    ));
}

/// Preserve upstream configuration values for the future runtime to interpret.
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
        assert_eq!(options.model.as_ref().map(ModelName::as_str), Some(model));
    }
}
