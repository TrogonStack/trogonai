use super::*;
use crate::ordinary_request::OrdinaryRequest;
use trogon_sandbox_contract::StdinMode;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn frame(operation: &OrdinaryOperation) -> Result<serde_json::Value, serde_json::Error> {
    serde_json::to_value(OrdinaryRequest {
        version: 1,
        execution_id: "e",
        nonce: "n",
        request_id: 4,
        operation,
    })
}

#[test]
fn a_signal_frame_carries_the_binding_and_the_signal() -> TestResult {
    let value = frame(&OrdinaryOperation::Signal { signal: 15 })?;
    assert_eq!(
        value,
        serde_json::json!({"version":1,"execution_id":"e","nonce":"n","request_id":4,"kind":"signal","signal":15})
    );
    Ok(())
}

#[test]
fn launch_and_stdin_frames_have_the_shapes_the_bridge_expects() -> TestResult {
    assert_eq!(
        frame(&OrdinaryOperation::Launch)?,
        serde_json::json!({"version":1,"execution_id":"e","nonce":"n","request_id":4,"kind":"launch"})
    );
    assert_eq!(
        frame(&OrdinaryOperation::Stdin {
            data: "aGk=".to_owned(),
            eof: true
        })?,
        serde_json::json!({"version":1,"execution_id":"e","nonce":"n","request_id":4,"kind":"stdin","data":"aGk=","eof":true})
    );
    Ok(())
}

#[test]
fn the_init_frame_sends_the_stdin_mode_as_a_boolean() -> TestResult {
    let init = |stdin| OrdinaryOperation::Init {
        config: crate::ordinary_configuration::OrdinaryConfiguration {
            argv: vec!["true".to_owned()],
            env: Default::default(),
            cwd: None,
            timeout_ms: Some(5000),
            user: None,
            stdin,
        },
    };
    assert_eq!(frame(&init(StdinMode::Interactive))?["config"]["interactive"], true);
    assert_eq!(frame(&init(StdinMode::Closed))?["config"]["interactive"], false);
    assert_eq!(frame(&init(StdinMode::Closed))?["config"]["timeout_ms"], 5000);
    Ok(())
}

#[test]
fn operations_report_the_name_and_signal_the_acknowledgement_must_echo() {
    assert_eq!(OrdinaryOperation::Launch.name(), "launch");
    assert_eq!(OrdinaryOperation::Signal { signal: 9 }.name(), "signal");
    assert_eq!(OrdinaryOperation::Signal { signal: 9 }.signal(), Some(9));
    assert_eq!(OrdinaryOperation::Launch.signal(), None);
}
