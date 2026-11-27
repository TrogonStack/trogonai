use serde_json::{Value, json};
use trogon_sandbox_client::BusMessage;
use trogon_sandbox_contract::{LeaseError, TerminalResponse};

use super::support::call_with;
use crate::policy::{CommandPolicy, Policy};
use crate::testing::{McpSession, Reply, ScriptedBus, TestResult, call_result_text, test_server};

fn frame(value: Value) -> Result<BusMessage, serde_json::Error> {
    Ok(BusMessage {
        subject: "sandbox.terminal.t1.output".into(),
        payload: serde_json::to_vec(&value)?.into(),
    })
}

fn done() -> Result<Reply, serde_json::Error> {
    Reply::json(&TerminalResponse::Done)
}

#[tokio::test]
async fn a_terminal_is_opened_typed_into_read_resized_and_closed() -> TestResult {
    let frames = vec![
        frame(json!({"type": "data", "seq": 1, "data": "aGk="}))?,
        frame(json!({"type": "exit", "seq": 2, "exit_code": 0}))?,
    ];
    let bus = ScriptedBus::scripted(vec![done()?]).with_subscribe_items(frames);
    let session = McpSession::start(test_server(bus, Policy::default())?).await?;
    let lease = json!({"lease_id": "l1", "terminal_id": "t1"});
    let with = |extra: Value| -> Value {
        let mut args = lease.clone();
        if let (Some(target), Value::Object(more)) = (args.as_object_mut(), extra) {
            target.extend(more);
        }
        args
    };

    let opened = session
        .call(
            "sandbox_terminal_open",
            with(json!({"cols": 80, "rows": 24, "command": ["sh"]})),
        )
        .await?;
    assert_ne!(opened.is_error, Some(true));
    assert!(call_result_text(&opened)?.contains("t1"));

    let typed = session
        .call("sandbox_terminal_write", with(json!({"text": "ls\r"})))
        .await?;
    assert_ne!(typed.is_error, Some(true));
    let raw = session
        .call("sandbox_terminal_write", with(json!({"data_base64": "bHMN"})))
        .await?;
    assert_ne!(raw.is_error, Some(true));

    let read = session
        .call(
            "sandbox_terminal_read",
            with(json!({"wait_ms": 1000, "max_bytes": 1024})),
        )
        .await?;
    let text = call_result_text(&read)?;
    assert!(text.contains("hi") && text.contains("\"exited\":true"), "{text}");

    let resized = session
        .call("sandbox_terminal_resize", with(json!({"cols": 100, "rows": 30})))
        .await?;
    assert_ne!(resized.is_error, Some(true));
    let closed = session.call("sandbox_terminal_close", lease.clone()).await?;
    assert_ne!(closed.is_error, Some(true));
    let gone = session.call("sandbox_terminal_read", lease.clone()).await?;
    assert_eq!(gone.is_error, Some(true));
    session.finish().await?;
    Ok(())
}

#[tokio::test]
async fn a_terminal_gets_a_fresh_id_when_none_is_named() -> TestResult {
    let outcome = call_with(
        Policy::default(),
        "sandbox_terminal_open",
        json!({"lease_id": "l1"}),
        vec![done()?],
    )
    .await?;
    assert!(!outcome.is_error());
    assert!(outcome.text()?.contains("terminal_id"));
    Ok(())
}

#[tokio::test]
async fn bad_terminal_inputs_never_reach_the_bus() -> TestResult {
    let replies = || -> Result<Vec<Reply>, serde_json::Error> { Ok(vec![done()?]) };
    let cases = [
        (
            "sandbox_terminal_open",
            json!({"lease_id": "l1", "terminal_id": "bad id!"}),
        ),
        ("sandbox_terminal_open", json!({"lease_id": "l1", "cols": 0})),
        ("sandbox_terminal_open", json!({"lease_id": "l1", "rows": 0})),
        (
            "sandbox_terminal_open",
            json!({"lease_id": "l1", "command": ["a\u{0}b"]}),
        ),
        ("sandbox_terminal_write", json!({"lease_id": "l1", "terminal_id": "t1"})),
        (
            "sandbox_terminal_write",
            json!({"lease_id": "l1", "terminal_id": "t1", "text": "a", "data_base64": "YQ=="}),
        ),
        (
            "sandbox_terminal_write",
            json!({"lease_id": "l1", "terminal_id": "t1", "data_base64": "***"}),
        ),
        (
            "sandbox_terminal_write",
            json!({"lease_id": "l1", "terminal_id": "bad id!", "text": "a"}),
        ),
        (
            "sandbox_terminal_resize",
            json!({"lease_id": "l1", "terminal_id": "t1", "cols": 0, "rows": 5}),
        ),
        (
            "sandbox_terminal_resize",
            json!({"lease_id": "l1", "terminal_id": "t1", "cols": 5, "rows": 0}),
        ),
        (
            "sandbox_terminal_read",
            json!({"lease_id": "l1", "terminal_id": "bad id!"}),
        ),
        (
            "sandbox_terminal_read",
            json!({"lease_id": "l1", "terminal_id": "never-opened"}),
        ),
        (
            "sandbox_terminal_close",
            json!({"lease_id": "l1", "terminal_id": "bad id!"}),
        ),
    ];
    for (tool, args) in cases {
        let outcome = call_with(Policy::default(), tool, args.clone(), replies()?).await?;
        assert!(outcome.is_error(), "{tool} {args}");
        assert!(outcome.sent.is_empty(), "{tool} {args}");
    }
    Ok(())
}

#[tokio::test]
async fn a_command_policy_refuses_terminals() -> TestResult {
    let policy = Policy {
        commands: CommandPolicy::new(&["^rm".to_string()], &[])?,
        ..Policy::default()
    };
    let outcome = call_with(
        policy,
        "sandbox_terminal_open",
        json!({"lease_id": "l1"}),
        vec![done()?],
    )
    .await?;
    assert!(outcome.is_error());
    assert!(outcome.sent.is_empty());
    Ok(())
}

#[tokio::test]
async fn a_refused_open_forgets_the_terminal() -> TestResult {
    let replies = vec![
        Reply::json(&TerminalResponse::Unsupported)?,
        Reply::json(&TerminalResponse::Denied(LeaseError::NoProviderAvailable))?,
    ];
    let bus = ScriptedBus::scripted(replies);
    let session = McpSession::start(test_server(bus, Policy::default())?).await?;
    let args = json!({"lease_id": "l1", "terminal_id": "t1"});
    let unsupported = session.call("sandbox_terminal_open", args.clone()).await?;
    assert_eq!(unsupported.is_error, Some(true));
    let denied = session
        .call(
            "sandbox_terminal_write",
            json!({"lease_id": "l1", "terminal_id": "t1", "text": "a"}),
        )
        .await?;
    assert_eq!(denied.is_error, Some(true));
    let read = session
        .call("sandbox_terminal_read", json!({"lease_id": "l1", "terminal_id": "t1"}))
        .await?;
    assert_eq!(read.is_error, Some(true));
    session.finish().await?;
    Ok(())
}

#[tokio::test]
async fn closing_forgets_the_terminal_even_when_the_broker_refuses() -> TestResult {
    let failed = serde_json::to_value(TerminalResponse::Denied(LeaseError::NoProviderAvailable))?;
    let outcome = call_with(
        Policy::default(),
        "sandbox_terminal_close",
        json!({"lease_id": "l1", "terminal_id": "t1"}),
        vec![Reply::json(&failed)?],
    )
    .await?;
    assert!(outcome.is_error());
    Ok(())
}
