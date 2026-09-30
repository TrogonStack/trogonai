use super::*;
use serde_json::json;

#[test]
fn from_principal_reads_valid_spicedb_subject_unchanged() {
    let p = SpiceDbPrincipal(json!({"spicedb_subject": "user/alice"}));
    assert_eq!(CallerId::from_principal(&p).as_str(), "user/alice");
}

#[test]
fn from_principal_without_subject_claim_is_placeholder() {
    let p = SpiceDbPrincipal(json!({}));
    assert_eq!(CallerId::from_principal(&p).as_str(), DEFAULT_PUSH_DLQ_CALLER_SEGMENT);
}

#[test]
fn from_principal_percent_encodes_dotted_subject() {
    let p = SpiceDbPrincipal(json!({"spicedb_subject": " u1.id "}));
    assert_eq!(CallerId::from_principal(&p).as_str(), "u1%2Eid");
}

#[test]
fn distinct_subjects_map_to_distinct_segments() {
    let dotted = SpiceDbPrincipal(json!({"spicedb_subject": "user.alice"}));
    let underscored = SpiceDbPrincipal(json!({"spicedb_subject": "user_alice"}));
    let dotted_id = CallerId::from_principal(&dotted);
    let underscored_id = CallerId::from_principal(&underscored);
    assert_eq!(dotted_id.as_str(), "user%2Ealice");
    assert_eq!(underscored_id.as_str(), "user_alice");
    assert_ne!(dotted_id.as_str(), underscored_id.as_str());
}

#[test]
fn from_principal_percent_encodes_ascii_control_chars() {
    let p = SpiceDbPrincipal(json!({"spicedb_subject": "a\u{1}b"}));
    assert_eq!(CallerId::from_principal(&p).as_str(), "a%01b");
}

#[test]
fn from_principal_percent_encodes_literal_percent() {
    let p = SpiceDbPrincipal(json!({"spicedb_subject": "100%done"}));
    assert_eq!(CallerId::from_principal(&p).as_str(), "100%25done");
}

#[test]
fn from_str_percent_encodes_forbidden_characters() {
    assert_eq!(CallerId::from("valid-caller").as_str(), "valid-caller");
    assert_eq!(CallerId::from("has.dot").as_str(), "has%2Edot");
}

#[test]
fn default_matches_env_placeholder_literal() {
    assert_eq!(CallerId::default().as_str(), DEFAULT_PUSH_DLQ_CALLER_SEGMENT);
}

#[test]
fn resolve_push_dlq_caller_id_absent_principal_uses_fallback() {
    let fallback = CallerId::from("env-seg");
    assert_eq!(resolve_push_dlq_caller_id(None, &fallback).as_str(), "env-seg");
}

#[test]
fn resolve_push_dlq_caller_id_with_valid_subject_uses_it_unchanged() {
    let p = SpiceDbPrincipal(json!({"spicedb_subject": "p-q"}));
    assert_eq!(
        resolve_push_dlq_caller_id(Some(&p), &CallerId::default()).as_str(),
        "p-q"
    );
}

#[test]
fn resolve_push_dlq_caller_id_with_subject_needing_encoding_encodes_it() {
    let p = SpiceDbPrincipal(json!({"spicedb_subject": "p.q"}));
    assert_eq!(
        resolve_push_dlq_caller_id(Some(&p), &CallerId::default()).as_str(),
        "p%2Eq"
    );
}

#[test]
fn resolve_push_dlq_caller_id_without_subject_uses_fallback() {
    let p = SpiceDbPrincipal(json!({}));
    assert_eq!(
        resolve_push_dlq_caller_id(Some(&p), &CallerId::default()).as_str(),
        DEFAULT_PUSH_DLQ_CALLER_SEGMENT
    );
}

#[test]
fn default_caller_id_matches_segment_constant() {
    assert_eq!(CallerId::default().to_string(), DEFAULT_PUSH_DLQ_CALLER_SEGMENT);
}

#[test]
fn resolve_push_dlq_caller_id_whitespace_only_subject_uses_fallback() {
    let fallback = CallerId::from("env-seg");
    for blank in ["   ", "\t", "\n"] {
        let p = SpiceDbPrincipal(json!({"spicedb_subject": blank}));
        assert_eq!(
            resolve_push_dlq_caller_id(Some(&p), &fallback).as_str(),
            "env-seg",
            "whitespace-only subject {blank:?} must route to the configured fallback"
        );
    }
}
