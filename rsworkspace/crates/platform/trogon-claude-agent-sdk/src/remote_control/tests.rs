#![cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    allow(
        serde_json_macro,
        reason = "Tests decode work secrets built from exact JSON payloads."
    )
)]

use super::*;
use serde_json::json;

fn encode(text: &str) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut encoded = String::new();
    for chunk in text.as_bytes().chunks(3) {
        let mut group = 0_u32;
        for (index, byte) in chunk.iter().enumerate() {
            group |= u32::from(*byte) << (16 - 8 * index);
        }
        for index in 0..=chunk.len() {
            let sextet = (group >> (18 - 6 * index)) & 0x3f;
            encoded.push(char::from(ALPHABET[sextet as usize]));
        }
    }
    encoded
}

fn secret(token: &str) -> String {
    encode(&json!({ "session_ingress_token": token }).to_string())
}

fn token(claims: &Value) -> String {
    format!("sk-ant-si-header.{}.signature", encode(&claims.to_string()))
}

#[test]
fn a_work_secret_names_the_session_in_its_ingress_token() {
    let named = secret(&token(&json!({ "session_id": "cse_abcd1234" })));
    assert_eq!(work_secret_session_id(&named).as_deref(), Some("cse_abcd1234"));
    let bare = secret("header.e30.signature");
    assert_eq!(work_secret_session_id(&bare), None);
}

#[test]
fn a_token_without_the_prefix_is_read_the_same_way() {
    let claims = encode(r#"{"session_id":"s1"}"#);
    assert_eq!(
        work_secret_session_id(&secret(&format!("h.{claims}.s"))).as_deref(),
        Some("s1")
    );
}

#[test]
fn anything_else_has_no_session_id() {
    for secret in [
        "not json at all".to_owned(),
        encode("[]"),
        encode(r#"{"session_ingress_token":5}"#),
        secret("only.two"),
        secret("a.b.c.d"),
        secret("a..c"),
        secret(&token(&json!({ "session_id": "" }))),
        secret(&token(&json!({ "session_id": 7 }))),
        secret(&token(&json!([]))),
    ] {
        assert_eq!(work_secret_session_id(&secret), None, "{secret}");
    }
}

#[test]
fn base64_decoding_matches_node_buffer() {
    for (text, expected) in [
        ("QUJD", "ABC"),
        ("QU=JD", "A"),
        ("Q UJD", "ABC"),
        ("QUJDRA", "ABCD"),
        ("QUJDR", "ABC"),
        ("QUJD=RA==", "ABC"),
        ("=QUJD", ""),
        ("QUJD\nRA", "ABCD"),
        ("QUJDREV", "ABCDE"),
    ] {
        assert_eq!(decode_base64_lenient(text), expected.as_bytes(), "{text}");
    }
    assert_eq!(decode_base64_lenient("Q-_+/A"), [0x43, 0xef, 0xfe, 0xfc]);
}

#[test]
fn sessions_match_when_equal_or_sharing_a_suffix_of_four_characters() {
    assert!(same_session("cse_abcd", "cse_abcd"));
    assert!(same_session("session_1234", "other_1234"));
    assert!(same_session("1234", "x_1234"));
    assert!(!same_session("a_123", "b_123"));
    assert!(!same_session("a_1234", "a_12345"));
    assert!(!same_session("", "x"));
}

#[test]
fn a_requested_session_of_another_type_fails_like_the_javascript_engine() {
    assert_eq!(same_requested_session("a_1234", Some(&json!("b_1234"))), Ok(true));
    assert_eq!(same_requested_session("a_1234", Some(&json!(["a_1234"]))), Ok(false));
    assert_eq!(
        same_requested_session("a_1234", None),
        Err("Cannot read properties of undefined (reading 'slice')".to_owned())
    );
    assert_eq!(
        same_requested_session("a_1234", Some(&Value::Null)),
        Err("Cannot read properties of null (reading 'slice')".to_owned())
    );
    for value in [json!(5), json!(true), json!({})] {
        assert_eq!(
            same_requested_session("a_1234", Some(&value)),
            Err("t.lastIndexOf is not a function".to_owned())
        );
    }
}

fn refresher() -> Arc<dyn WorkSecretRefresher> {
    Arc::new(|_: crate::WorkSecretRefreshRequest| async { Ok::<_, CallbackError>(None) })
}

#[test]
fn the_answerer_needs_a_secret_a_refresher_a_session_and_an_enable() {
    let named = RemoteControlOptions {
        work_secret: Some("opaque".into()),
        refresh_work_secret: Some(refresher()),
        reattach_session_id: Some("cse_1234".into()),
        keep_session_on_exit: None,
    };
    assert_eq!(
        named.answerer(true).map(|answerer| answerer.session_id).as_deref(),
        Some("cse_1234")
    );
    assert!(named.answerer(false).is_none());
    let mut unnamed = named.clone();
    unnamed.reattach_session_id = Some(String::new());
    assert!(unnamed.answerer(true).is_none());
    unnamed.work_secret = Some(secret(&token(&json!({ "session_id": "cse_9999" }))).into());
    assert_eq!(
        unnamed.answerer(true).map(|answerer| answerer.session_id).as_deref(),
        Some("cse_9999")
    );
    let mut without_refresher = named.clone();
    without_refresher.refresh_work_secret = None;
    assert!(without_refresher.answerer(true).is_none());
    let mut without_secret = named;
    without_secret.work_secret = None;
    assert!(without_secret.answerer(true).is_none());
}

#[test]
fn the_newest_call_decides_which_answerer_stays() {
    let state = HostState::default();
    let answerer = |session_id: &str| WorkSecretAnswerer {
        session_id: session_id.to_owned(),
        refresh: refresher(),
    };
    let (first, previous) = state.begin_remote_control(Some(answerer("a_1111")));
    assert!(previous.is_none());
    let (second, previous) = state.begin_remote_control(Some(answerer("b_2222")));
    state.remote_control_failed(first);
    assert_eq!(state.answerer().map(|a| a.session_id).as_deref(), Some("b_2222"));
    assert!(state.remote_control_enabled(first, Some(&json!({"bridge_session_id":"a_1111"})), [None, None]));
    assert_eq!(state.answerer().map(|a| a.session_id).as_deref(), Some("b_2222"));
    assert!(!state.remote_control_enabled(second, None, [None, None]));
    assert!(!state.remote_control_enabled(second, Some(&Value::Null), [None, None]));
    assert_eq!(state.answerer().map(|a| a.session_id).as_deref(), Some("b_2222"));
    let bridge = json!({"bridge_session_id":"zz_1111"});
    assert!(state.remote_control_enabled(second, Some(&bridge), [state.answerer(), previous]));
    assert_eq!(state.answerer().map(|a| a.session_id).as_deref(), Some("a_1111"));
    assert!(state.remote_control_enabled(second, Some(&json!({"bridge_session_id":5})), [state.answerer(), None]));
    assert!(state.answerer().is_none());
}

#[test]
fn remote_control_options_debug_without_the_secret_or_the_closure() {
    let options = RemoteControlOptions {
        work_secret: Some("opaque".into()),
        refresh_work_secret: Some(refresher()),
        ..RemoteControlOptions::default()
    };
    let debug = format!("{options:?}");
    assert!(!debug.contains("opaque"), "{debug}");
    assert!(debug.contains("WorkSecretRefresher"), "{debug}");
}
