use serde_json::Value;

use super::*;

/// What the bundle's own regular expressions and checks, and the path module of the platform, answered for each input
/// (`.config/mise/lib/claude-sdk-freshness/bridge_create_session_platform_goldens.mjs` writes the goldens).
const GOLDENS: &str = include_str!("../tests/fixtures/bridge_create_session/flavor.json");

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn answer(flavor: Flavor, call: &[Value]) -> Result<Value, String> {
    let arg = |index: usize| call.get(index).and_then(Value::as_str).unwrap_or_default();
    Ok(match arg(0) {
        "sep" => Value::String(flavor.sep().to_string()),
        "rooted" => Value::Bool(flavor.rooted(arg(1))),
        "rootLen" => Value::from(flavor.root_len(arg(1))),
        "split" => Value::from(flavor.split(arg(1))),
        "ends" => Value::Bool(flavor.ends_with_separator(arg(1))),
        "server" => server_name(arg(1)).map_or(Value::Null, Value::String),
        "device" => Value::Bool(device_prefix(arg(1))),
        "dots" => Value::Bool(dot_segment(arg(1))),
        "nt" => Value::Bool(nt_prefix(arg(1))),
        "two" => Value::Bool(two_separators(arg(1)) || nt_prefix(arg(1))),
        "share" => Value::Bool(share_refused(arg(1), arg(2))),
        "dirname" => Value::String(flavor.dirname(arg(1))),
        "join" => Value::String(flavor.join(arg(1), arg(2))),
        "resolve" => Value::String(flavor.resolve(arg(1), arg(2))),
        "resolveAlone" => Value::String(flavor.resolve_alone(arg(1))),
        other => return Err(format!("unknown operation {other}")),
    })
}

#[test]
fn answers_path_syntax_like_the_upstream_helpers() -> TestResult {
    let cases: Vec<Value> = serde_json::from_str(GOLDENS).unwrap_or_default();
    assert_eq!(cases.len(), 2);
    for case in &cases {
        let flavor = Flavor::of(if case["platform"] == "win32" {
            Platform::Win32
        } else {
            Platform::Linux
        });
        let calls = case["args"].as_array().cloned().unwrap_or_default();
        let expected = case["expect"].as_array().cloned().unwrap_or_default();
        assert!(calls.len() > 500);
        assert_eq!(calls.len(), expected.len());
        for (call, want) in calls.iter().zip(&expected) {
            let call = call.as_array().map(Vec::as_slice).unwrap_or_default();
            assert_eq!(answer(flavor, call)?, *want, "{} {call:?}", case["name"]);
        }
    }
    Ok(())
}
