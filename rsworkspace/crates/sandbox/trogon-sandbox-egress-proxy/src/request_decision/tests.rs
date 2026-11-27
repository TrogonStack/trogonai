use super::*;

fn headers(list: &[(&str, &str)]) -> Vec<(String, String)> {
    list.iter()
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect()
}

#[test]
fn removal_covers_every_spelling_of_a_name() -> Result<(), Box<dyn std::error::Error>> {
    let edits = HeaderEdits::new().remove("x-api-key")?;
    let mut list = headers(&[
        ("X_Api_Key", "a"),
        ("x.api.key", "b"),
        ("X-API-KEY", "c"),
        ("accept", "d"),
    ]);
    edits.apply(&mut list);
    assert_eq!(list, headers(&[("accept", "d")]));
    Ok(())
}

#[test]
fn a_set_replaces_the_client_spellings_and_keeps_every_value_in_order() -> Result<(), Box<dyn std::error::Error>> {
    let edits = HeaderEdits::new().set("X-Tag", "one")?.set("x_tag", "two")?;
    let mut list = headers(&[("x-tag", "client"), ("accept", "d")]);
    edits.apply(&mut list);
    assert_eq!(list, headers(&[("accept", "d"), ("x-tag", "one"), ("x-tag", "two")]));
    Ok(())
}

#[test]
fn framing_headers_are_neither_removed_nor_set() -> Result<(), Box<dyn std::error::Error>> {
    let edits = HeaderEdits::new()
        .remove("content-length")?
        .set("Host", "evil.example")?
        .set("transfer-encoding", "chunked")?;
    let mut list = headers(&[("host", "a.test"), ("content-length", "3")]);
    edits.apply(&mut list);
    assert_eq!(list, headers(&[("host", "a.test"), ("content-length", "3")]));
    Ok(())
}

#[test]
fn names_and_values_are_validated() -> Result<(), Box<dyn std::error::Error>> {
    assert!(matches!(
        HeaderEdits::new().remove("bad name"),
        Err(MalformedDecisionError::HeaderName(name)) if name == "bad name"
    ));
    assert!(matches!(
        HeaderEdits::new().set("x", "a\r\nb"),
        Err(MalformedDecisionError::HeaderValue)
    ));
    assert!(matches!(
        HeaderEdits::new().set("x", "\u{100}"),
        Err(MalformedDecisionError::HeaderValue)
    ));
    assert!(HeaderEdits::new().set("x", "a\tb\u{ff}").is_ok());
    assert!(DenyMark::parse("not a token").is_err());
    assert!(DenyMark::parse("decider").is_ok());
    Ok(())
}

#[test]
fn a_status_outside_400_to_599_answers_403() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(RequestDenial::new().answer_status(), 403);
    assert_eq!(RequestDenial::new().status(200).answer_status(), 403);
    assert_eq!(RequestDenial::new().status(600).answer_status(), 403);
    assert_eq!(RequestDenial::new().status(451).answer_status(), 451);
    Ok(())
}

#[test]
fn the_denial_answer_carries_the_reason_and_the_mark() -> Result<(), Box<dyn std::error::Error>> {
    let at = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
    let kept = AnsweredRequest::new("GET", false);
    let answer = RequestDenial::new()
        .status(418)
        .reason("no tea")
        .mark(DenyMark::parse("tea")?)
        .answer(kept, at);
    assert_eq!(
        String::from_utf8(answer)?,
        "HTTP/1.1 418 I'm a Teapot\r\nContent-Type: text/plain\r\nX-Proxy-Error: blocked-by-egress-proxy\r\nDate: Tue, 14 Nov 2023 22:13:20 GMT\r\nConnection: keep-alive\r\nKeep-Alive: timeout=5\r\nTransfer-Encoding: chunked\r\n\r\n7\r\nno tea\n\r\n0\r\n\r\n"
    );
    let default = RequestDenial::new().answer(kept, at);
    assert_eq!(
        String::from_utf8(default)?,
        "HTTP/1.1 403 Forbidden\r\nContent-Type: text/plain\r\nX-Proxy-Error: blocked-by-egress-proxy\r\nDate: Tue, 14 Nov 2023 22:13:20 GMT\r\nConnection: keep-alive\r\nKeep-Alive: timeout=5\r\nTransfer-Encoding: chunked\r\n\r\n19\r\ndenied by sandbox policy\n\r\n0\r\n\r\n"
    );
    let closing = RequestDenial::new().answer(AnsweredRequest::new("GET", true), at);
    assert_eq!(
        String::from_utf8(closing)?,
        "HTTP/1.1 403 Forbidden\r\nContent-Type: text/plain\r\nX-Proxy-Error: blocked-by-egress-proxy\r\nDate: Tue, 14 Nov 2023 22:13:20 GMT\r\nConnection: close\r\nTransfer-Encoding: chunked\r\n\r\n19\r\ndenied by sandbox policy\n\r\n0\r\n\r\n"
    );
    Ok(())
}

#[tokio::test]
async fn the_deny_header_carries_the_decision_mark_or_denied() -> Result<(), Box<dyn std::error::Error>> {
    let at = std::time::UNIX_EPOCH;
    let kept = AnsweredRequest::new("HEAD", false);
    let marked = RequestDenial::new().mark(DenyMark::parse("tea")?);
    let (marked, unmarked) = crate::deny_header_style::DenyHeaderStyle::DenyReason
        .scope(async { (marked.answer(kept, at), RequestDenial::new().answer(kept, at)) })
        .await;
    assert!(String::from_utf8(marked)?.contains("\r\nX-Deny-Reason: tea\r\n"));
    assert!(String::from_utf8(unmarked)?.contains("\r\nX-Deny-Reason: denied\r\n"));
    Ok(())
}

#[test]
fn a_status_server_does_not_name_reads_unknown() -> Result<(), Box<dyn std::error::Error>> {
    let at = std::time::UNIX_EPOCH;
    let answer = RequestDenial::new()
        .status(499)
        .answer(AnsweredRequest::new("HEAD", false), at);
    assert!(String::from_utf8(answer)?.starts_with("HTTP/1.1 499 unknown\r\n"));
    Ok(())
}

#[test]
fn a_panicking_response_hook_is_contained() -> Result<(), Box<dyn std::error::Error>> {
    ResponseHook::new(|_, _| std::panic::resume_unwind(Box::new("boom"))).notify(200, &[]);
    Ok(())
}

#[test]
fn chained_hooks_both_run() -> Result<(), Box<dyn std::error::Error>> {
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let (a, b) = (Arc::clone(&seen), Arc::clone(&seen));
    let hook = ResponseHook::new(move |status, _| {
        if let Ok(mut seen) = a.lock() {
            seen.push(("a", status));
        }
    })
    .then(ResponseHook::new(move |status, _| {
        if let Ok(mut seen) = b.lock() {
            seen.push(("b", status));
        }
    }));
    hook.notify(204, &[]);
    let seen = seen.lock().map_err(|_| "the seen list was poisoned")?;
    assert_eq!(*seen, vec![("a", 204), ("b", 204)]);
    Ok(())
}

#[test]
fn a_set_with_a_bad_name_is_refused() -> Result<(), Box<dyn std::error::Error>> {
    assert!(matches!(
        HeaderEdits::new().set("bad name", "v"),
        Err(MalformedDecisionError::HeaderName(name)) if name == "bad name"
    ));
    Ok(())
}

#[test]
fn merged_edits_keep_every_removal_and_set() -> Result<(), Box<dyn std::error::Error>> {
    let mut edits = HeaderEdits::new().remove("a")?;
    edits.merge(HeaderEdits::new().remove("b")?.set("c", "1")?);
    let mut list = headers(&[("a", "x"), ("b", "y"), ("d", "z")]);
    edits.apply(&mut list);
    assert_eq!(list, headers(&[("d", "z"), ("c", "1")]));
    Ok(())
}

#[test]
fn hooks_show_a_fixed_debug_and_compare_by_identity() -> Result<(), Box<dyn std::error::Error>> {
    let hook = ResponseHook::new(|_, _| {});
    assert_eq!(format!("{hook:?}"), "ResponseHook");
    assert_eq!(hook, hook.clone());
    assert_ne!(hook, ResponseHook::new(|_, _| {}));
    Ok(())
}

#[test]
fn merged_allows_chain_their_hooks_and_keep_a_single_one() -> Result<(), Box<dyn std::error::Error>> {
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let (a, b) = (Arc::clone(&seen), Arc::clone(&seen));
    let first = RequestAllow::new()
        .edits(HeaderEdits::new().remove("a")?)
        .on_response(ResponseHook::new(move |status, _| {
            if let Ok(mut seen) = a.lock() {
                seen.push(("a", status));
            }
        }));
    let second = RequestAllow::new().on_response(ResponseHook::new(move |status, _| {
        if let Ok(mut seen) = b.lock() {
            seen.push(("b", status));
        }
    }));
    let mut merged = RequestAllow::new();
    assert!(merged.is_empty());
    merged.merge(first);
    merged.merge(second);
    assert!(!merged.is_empty());
    assert!(!merged.header_edits().is_empty());
    merged.response_hook().ok_or("a chained hook")?.notify(200, &[]);
    let seen = seen.lock().map_err(|_| "the seen list was poisoned")?;
    assert_eq!(*seen, vec![("a", 200), ("b", 200)]);
    Ok(())
}
