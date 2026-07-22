use super::*;
use serde_json::json;

#[test]
fn from_principal_reads_spicedb_subject_and_sanitizes() {
    let p = SpiceDbPrincipal(json!({"spicedb_subject": "user/al.ice"}));
    assert_eq!(CallerId::from_principal(&p).as_str(), "user/al_ice");
}

#[test]
fn from_principal_without_subject_claim_is_placeholder() {
    let p = SpiceDbPrincipal(json!({}));
    assert_eq!(CallerId::from_principal(&p).as_str(), DEFAULT_PUSH_DLQ_CALLER_SEGMENT);
}

#[test]
fn sanitization_recovers_single_segment_from_spaces_and_dots() {
    assert_eq!(
        CallerId::from_principal(&SpiceDbPrincipal(json!({"spicedb_subject": " u1.id "}))).as_str(),
        "u1_id"
    );
}

#[test]
fn from_principal_sanitizes_ascii_control_chars() {
    let p = SpiceDbPrincipal(json!({"spicedb_subject": "a\u{1}b"}));
    assert_eq!(CallerId::from_principal(&p).as_str(), "a_b");
}

#[test]
fn from_str_behaves_like_sanitizer() {
    assert_eq!(CallerId::from("_").as_str(), "_");
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
fn resolve_push_dlq_caller_id_with_subject_uses_sanitized_segment() {
    let p = SpiceDbPrincipal(json!({"spicedb_subject": "p.q"}));
    assert_eq!(
        resolve_push_dlq_caller_id(Some(&p), &CallerId::default()).as_str(),
        "p_q"
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
fn sanitize_subject_token_blank_string_returns_default_segment() {
    assert_eq!(sanitize_subject_token("").as_ref(), DEFAULT_PUSH_DLQ_CALLER_SEGMENT);
    assert_eq!(sanitize_subject_token("   ").as_ref(), DEFAULT_PUSH_DLQ_CALLER_SEGMENT);
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
