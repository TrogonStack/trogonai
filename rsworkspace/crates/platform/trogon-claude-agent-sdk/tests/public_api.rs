use std::{
    future::Future,
    pin::pin,
    task::{Context, Poll, Waker},
};

use trogon_claude_agent_sdk::{
    ClaudeAgentOptions, ClaudeSdkClient, CliExecutable, CliExecutableError, Error, ModelName, ModelNameError,
};

#[test]
fn connect_reports_unimplemented_runtime() {
    let mut connection = pin!(ClaudeSdkClient::connect(ClaudeAgentOptions::default()));
    let mut context = Context::from_waker(Waker::noop());
    assert!(matches!(
        connection.as_mut().poll(&mut context),
        Poll::Ready(Err(Error::NotImplemented))
    ));
}

#[test]
fn configuration_rejects_invalid_field_values() {
    assert!(matches!(CliExecutable::new(""), Err(CliExecutableError::Empty)));
    assert!(matches!(ModelName::new(""), Err(ModelNameError)));
    assert!(matches!(ModelName::new("model\nname"), Err(ModelNameError)));
}
