use super::*;

#[test]
fn json_pointer_from_dollar_path() {
    let root = serde_json::json!({"params": {"secret": "x"}});
    assert_eq!(
        value_at_path(&root, "$.params.secret"),
        Some(&serde_json::Value::String("x".into()))
    );
}

#[test]
fn bracket_index_in_dollar_path() {
    let root = serde_json::json!({"params": {"parts": [{"text": "x"}]}});
    assert_eq!(
        value_at_path(&root, "$.params.parts[0].text"),
        Some(&serde_json::Value::String("x".into()))
    );
}

#[test]
fn slash_pointer_resolves() {
    let root = serde_json::json!({"params": {"n": 1}});
    assert_eq!(
        value_at_path(&root, "/params/n"),
        Some(&serde_json::Value::Number(1.into()))
    );
}

#[test]
fn dollar_root_pointer_is_empty_string() {
    let root = serde_json::json!({"x": 1});
    assert_eq!(value_at_path(&root, "$."), Some(&root));
}

#[test]
fn unprefixed_path_returns_none() {
    let root = serde_json::json!({"x": 1});
    assert!(value_at_path(&root, "params.x").is_none());
}

#[test]
fn empty_bracket_segment_fails_resolution() {
    // `$.params.parts[].text` previously elided the empty bracket
    // and resolved to `/params/parts/text`, silently bypassing the
    // intended array index. The tokenizer now rejects empty brackets
    // so a malformed manifest path fails-closed at lookup.
    let root = serde_json::json!({"params": {"parts": [{"text": "x"}]}});
    assert!(value_at_path(&root, "$.params.parts[].text").is_none());
    assert!(to_json_pointer("$.params.parts[].text").is_none());
}

#[test]
fn unterminated_bracket_fails_resolution() {
    // An unterminated `[` consumed the rest of the input as if it
    // were an index, producing surprising pointers. The tokenizer
    // now rejects unterminated brackets explicitly.
    assert!(to_json_pointer("$.params.parts[0").is_none());
}

#[test]
fn trailing_dot_fails_resolution() {
    // `$.params.` previously normalized to `/params` by silently
    // dropping the trailing separator. Reject so a manifest typo
    // can't masquerade as the parent key.
    assert!(to_json_pointer("$.params.").is_none());
}

#[test]
fn bare_text_after_closing_bracket_fails_resolution() {
    // `$.parts[0]text` previously normalized to `/parts/0/text`
    // by appending the post-bracket text as if it were a new
    // segment. Reject so missing `.` separators fail-closed.
    assert!(to_json_pointer("$.parts[0]text").is_none());
}

#[test]
fn consecutive_dots_fail_resolution() {
    // `$..params` and `$.params..secret` previously collapsed to
    // the same pointer as `$.params` / `$.params.secret`,
    // hiding the JSONPath "recursive descent" intent behind a
    // benign-looking specific path. The tokenizer now rejects both.
    assert!(to_json_pointer("$..params").is_none());
    assert!(to_json_pointer("$.params..secret").is_none());
}

#[test]
fn whitespace_padded_segments_fail_resolution() {
    // Padding whitespace inside or around a segment almost always
    // means operator typo (e.g. `" params"` not `params`, `" 0 "`
    // not `0`). Accepting them silently produced surprising
    // pointers; the tokenizer now rejects them.
    assert!(to_json_pointer("$. params.secret").is_none());
    assert!(to_json_pointer("$.params.secret ").is_none());
    assert!(to_json_pointer("$.params[ 0 ].text").is_none());
    assert!(to_json_pointer("$.params[0 ].text").is_none());
}
