use super::*;

fn parse_suite(yaml: &str) -> FixtureSuite {
    serde_yaml::from_str(yaml).expect("valid fixture suite yaml")
}

#[test]
fn deserializes_allow_and_deny_verdicts() {
    let suite = parse_suite(
        r#"
suite: sample
bundle: policy
fixtures:
  - id: allow-case
    input:
      agent: { id: planner }
    expected_verdict: allow
  - id: deny-case
    input:
      agent: { id: planner }
    expected_verdict:
      deny:
        rule: some_rule
"#,
    );
    assert_eq!(suite.suite, "sample");
    assert_eq!(suite.fixtures.len(), 2);
    assert!(matches!(
        suite.fixtures[0].expected_verdict.to_outcome().expect("outcome"),
        ExpectedOutcome::Allow
    ));
    let ExpectedOutcome::Deny { rule } = suite.fixtures[1].expected_verdict.to_outcome().expect("outcome") else {
        panic!("expected deny outcome");
    };
    assert_eq!(rule.as_str(), "some_rule");
}

#[test]
fn defaults_request_method_to_message_send() {
    let suite = parse_suite(
        r#"
suite: sample
bundle: policy
fixtures:
  - id: default-method
    input:
      agent: { id: planner }
    expected_verdict: allow
"#,
    );
    let ctx = suite.fixtures[0]
        .input
        .to_evaluation_context()
        .expect("evaluation context");
    assert_eq!(ctx.request_method().as_str(), "message/send");
}

#[test]
fn builds_full_evaluation_context_from_fixture_input() {
    let suite = parse_suite(
        r#"
suite: sample
bundle: policy
fixtures:
  - id: full
    input:
      request:
        method: tasks/get
        params: { echo: 1 }
      caller: { id: caller-1 }
      agent: { id: planner }
      task: { id: task-9 }
      headers:
        x-tenant-id: acme
    expected_verdict: allow
"#,
    );
    let ctx = suite.fixtures[0]
        .input
        .to_evaluation_context()
        .expect("evaluation context");
    assert_eq!(ctx.request_method().as_str(), "tasks/get");
    assert_eq!(ctx.request_params(), &serde_json::json!({"echo": 1}));
    assert_eq!(ctx.caller_id().map(SpiceDbSubject::as_str), Some("caller-1"));
    assert_eq!(ctx.agent_id().as_str(), "planner");
    assert_eq!(ctx.task_id().map(A2aTaskId::as_str), Some("task-9"));
    assert_eq!(ctx.headers().get("x-tenant-id").map(String::as_str), Some("acme"));
}

#[test]
fn invalid_agent_id_is_rejected() {
    let suite = parse_suite(
        r#"
suite: sample
bundle: policy
fixtures:
  - id: bad-agent
    input:
      agent: { id: "" }
    expected_verdict: allow
"#,
    );
    let err = suite.fixtures[0]
        .input
        .to_evaluation_context()
        .expect_err("empty agent id rejected");
    assert!(matches!(err, FixtureInputError::AgentId(_)));
}

#[test]
fn invalid_request_method_is_rejected() {
    let suite = parse_suite(
        r#"
suite: sample
bundle: policy
fixtures:
  - id: bad-method
    input:
      request:
        method: not/a/real/method
      agent: { id: planner }
    expected_verdict: allow
"#,
    );
    let err = suite.fixtures[0]
        .input
        .to_evaluation_context()
        .expect_err("unknown method rejected");
    assert!(matches!(err, FixtureInputError::RequestMethod(_)));
}

#[test]
fn invalid_expected_rule_name_is_rejected() {
    let verdict: ExpectedVerdict = serde_yaml::from_str("deny:\n  rule: \"   \"\n").expect("parse deny verdict");
    let err = verdict.to_outcome().expect_err("blank rule name rejected");
    assert!(err.to_string().contains("expected_verdict.deny.rule"));
}
