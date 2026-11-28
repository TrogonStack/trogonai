use std::pin::Pin;
use std::task::{Context, Poll};

use serde::de::IntoDeserializer;
use serde::de::value::{BytesDeserializer, Error as ValueError};
use serde_json::json;
use tokio::io::{DuplexStream, ReadBuf, ReadHalf, WriteHalf, duplex, split};
use trogon_sandbox_contract::Host;
use trogon_sandbox_contract::HttpMethod;

use super::*;
use std::sync::atomic::AtomicBool;

use crate::framing::Framing;
use crate::request_body::{BodyRead, BodySource, BodyUnavailableError, RequestBody, StreamedBody};
use crate::request_filter::RequestScheme;

type TestError = Box<dyn std::error::Error>;
type TestResult = Result<(), TestError>;

/// Runs the proxy side and the decider side of one exchange together.
async fn joined<T, R>(
    proxy: impl Future<Output = Result<T, TestError>>,
    peer: impl Future<Output = Result<R, TestError>>,
) -> Result<(T, R), TestError> {
    let (proxy, peer) = tokio::join!(proxy, peer);
    Ok((proxy?, peer?))
}

fn object_of(value: &Value) -> Result<&serde_json::Map<String, Value>, TestError> {
    value.as_object().ok_or_else(|| "expected an object".into())
}

fn object_of_mut(value: &mut Value) -> Result<&mut serde_json::Map<String, Value>, TestError> {
    value.as_object_mut().ok_or_else(|| "expected an object".into())
}

struct Peer {
    reader: ReadHalf<DuplexStream>,
    writer: WriteHalf<DuplexStream>,
}

impl Peer {
    async fn read(&mut self) -> Result<Value, TestError> {
        let frame = read_frame(&mut self.reader).await?;
        parse_strict(&frame).ok_or_else(|| "expected an object".into())
    }

    async fn read_unbounded(&mut self) -> Result<Value, TestError> {
        let mut length = [0u8; 4];
        self.reader.read_exact(&mut length).await?;
        let mut frame = vec![0u8; u32::from_be_bytes(length) as usize];
        self.reader.read_exact(&mut frame).await?;
        parse_strict(&frame).ok_or_else(|| "expected an object".into())
    }

    async fn send(&mut self, message: Value) -> Result<(), TestError> {
        self.writer.write_all(&encode_frame(&message)?).await?;
        Ok(())
    }

    async fn send_raw(&mut self, bytes: &[u8]) -> Result<(), TestError> {
        let mut frame = (bytes.len() as u32).to_be_bytes().to_vec();
        frame.extend_from_slice(bytes);
        self.writer.write_all(&frame).await?;
        Ok(())
    }

    async fn verdict(&mut self, id: u64, rest: Value) -> Result<(), TestError> {
        let mut message = json!({"t": "verdict", "id": id});
        object_of_mut(&mut message)?.extend(object_of(&rest)?.clone());
        self.send(message).await
    }
}

fn hello() -> Value {
    json!({"t": "hello", "proto": 1, "allowedDomains": ["api.github.com"], "deniedDomains": []})
}

async fn open_with(
    options: DeciderOptions,
    hello: Value,
) -> Result<(Peer, Result<(DeciderHello, FramedDecider), String>), TestError> {
    let (proxy_in, decider_out) = duplex(1 << 20);
    let (decider_in, proxy_out) = duplex(1 << 20);
    let (reader, _) = split(proxy_in);
    let (_, writer) = split(proxy_out);
    let (peer_reader, _) = split(decider_in);
    let (_, peer_writer) = split(decider_out);
    let mut peer = Peer {
        reader: peer_reader,
        writer: peer_writer,
    };
    let opened = tokio::spawn(FramedDecider::open(reader, writer, options));
    assert_eq!(peer.read().await?, json!({"t": "hello", "proto": 1}));
    peer.send(hello).await?;
    Ok((peer, opened.await?))
}

async fn open(options: DeciderOptions) -> Result<(Peer, FramedDecider), TestError> {
    let (peer, opened) = open_with(options, hello()).await?;
    Ok((peer, opened?.1))
}

struct Asked<'a> {
    method: &'a str,
    path: &'a str,
    query: Option<&'a str>,
    sni: Option<&'a str>,
    headers: Vec<(String, String)>,
    body: Option<&'a [u8]>,
    source: Option<&'a dyn BodySource>,
}

type BodyOutcome = Result<BodyRead, BodyUnavailableError>;

struct Gated(Mutex<Option<oneshot::Receiver<BodyOutcome>>>);

impl Gated {
    fn new() -> (Self, oneshot::Sender<BodyOutcome>) {
        let (tx, rx) = oneshot::channel();
        (Self(Mutex::new(Some(rx))), tx)
    }
}

#[async_trait]
impl BodySource for Gated {
    async fn read(&self, _max: usize) -> BodyOutcome {
        let rx = self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).take();
        match rx {
            Some(rx) => rx.await.unwrap_or(Err(BodyUnavailableError::Unreadable)),
            None => Err(BodyUnavailableError::Unreadable),
        }
    }
}

async fn until(mut cond: impl FnMut() -> bool) {
    for _ in 0..400 {
        if cond() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(cond());
}

impl Default for Asked<'_> {
    fn default() -> Self {
        Self {
            method: "POST",
            path: "/repos",
            query: None,
            sni: None,
            headers: vec![
                ("Host".to_string(), "api.github.com".to_string()),
                ("X-One".to_string(), "1".to_string()),
            ],
            body: None,
            source: None,
        }
    }
}

async fn decide(decider: &FramedDecider, asked: &Asked<'_>) -> Result<RequestDecision, TestError> {
    let host = Host::new("api.github.com")?;
    let method = HttpMethod::new(asked.method)?;
    Ok(decider
        .decide(&RequestView {
            host: &host,
            port: 443,
            method: &method,
            path: asked.path,
            query: asked.query,
            sni: asked.sni,
            headers: &asked.headers,
            body: asked
                .source
                .map(RequestBody::streamed)
                .or(asked.body.map(|body| RequestBody::new(body, false))),
            scheme: RequestScheme::Https,
            forms: None,
        })
        .await)
}

fn status_of(decision: &RequestDecision) -> Result<(u16, String), TestError> {
    let RequestDecision::DenyWith(denial) = decision else {
        return Err(format!("expected a denial, got {decision:?}").into());
    };
    Ok((denial.answer_status(), denial.denial_reason().to_string()))
}

async fn answered(rest: Value) -> Result<RequestDecision, TestError> {
    let (mut peer, decider) = open(DeciderOptions::default()).await?;
    let asked = Asked::default();
    let (decision, ()) = joined(decide(&decider, &asked), async {
        let req = peer.read().await?;
        peer.verdict(req["id"].as_u64().ok_or("expected an id")?, rest).await?;
        Ok(())
    })
    .await?;
    Ok(decision)
}

#[tokio::test]
async fn the_hello_carries_the_decider_egress_lists() -> TestResult {
    let hello = json!({"t": "hello", "proto": 1, "allowedDomains": ["a.example", "*.b.example:443", "10.0.0.1", "[::1]:80"], "deniedDomains": ["c.example"]});
    let (_, opened) = open_with(DeciderOptions::default(), hello).await?;
    let (hello, _) = opened?;
    assert_eq!(hello.allowed_domains.len(), 4);
    assert_eq!(hello.denied_domains, vec!["c.example".to_string()]);
    Ok(())
}

#[tokio::test]
async fn a_hello_that_breaks_the_protocol_ends_the_exchange() -> TestResult {
    for bad in [
        json!({"t": "hello", "proto": 2, "allowedDomains": [], "deniedDomains": []}),
        json!({"t": "hello", "proto": 1, "allowedDomains": [], "deniedDomains": [], "extra": 1}),
        json!({"t": "hello", "proto": 1, "allowedDomains": ["a.example", "a.example"], "deniedDomains": []}),
        json!({"t": "hello", "proto": 1, "allowedDomains": ["https://a.example"], "deniedDomains": []}),
        json!({"t": "hello", "proto": 1, "allowedDomains": []}),
        json!({"t": "verdict", "id": 1, "action": "deny", "status": 403}),
    ] {
        let (_, opened) = open_with(DeciderOptions::default(), bad.clone()).await?;
        assert_eq!(opened.err().as_deref(), Some("decider unavailable: bad hello"), "{bad}");
    }
    Ok(())
}

#[test]
fn egress_entries_are_names_wildcards_and_ip_literals() -> TestResult {
    for fine in [
        "example.com",
        "*.example.com",
        "example.com:8080",
        "a-b.example",
        "1.2.3.4",
        "1.2.3.4:80",
        "::1",
        "[::1]",
        "[::1]:443",
        "localhost",
        "xn--d1acufc.xn--p1ai",
    ] {
        assert!(is_egress_entry(fine), "{fine}");
    }
    for bad in [
        "",
        "Example.com",
        "*.com",
        "*",
        "example.com:0",
        "example.com:65536",
        "example.com:080",
        "http://example.com",
        "example.com/path",
        "a b.example",
        "-a.example",
        "a-.example",
        "1.2.3",
        "1.2.3.4.5",
        "123.456",
        "[::1",
        "[::1]x",
        "[1.2.3.4]:80",
        "[::1]:",
        "a..example",
        "user@example.com",
        "example.com.",
        "*.example.com:99999",
    ] {
        assert!(!is_egress_entry(bad), "{bad}");
    }
    assert!(!is_egress_entry(&format!("{}.com", "a".repeat(64))));
    assert!(!is_egress_entry(&format!("{}.com", ["a"; 130].join("."))));
    Ok(())
}

#[test]
fn strict_json_refuses_what_a_lenient_parser_accepts() -> TestResult {
    assert!(parse_strict(br#"{"a":1,"b":[1,"x",{"c":true}]}"#).is_some());
    for bad in [
        &br#"{"a":null}"#[..],
        br#"{"a":[null]}"#,
        br#"{"a":1,"a":2}"#,
        br#"{"o":{"k":1,"k":1}}"#,
        br#"[1]"#,
        br#""text""#,
        br#"1"#,
        br#"{"a":1} {"b":2}"#,
        br#"{"a":1}x"#,
        b"{\"a\":\"\xff\"}",
        br#"{"a":"\ud800"}"#,
        b"",
    ] {
        assert!(parse_strict(bad).is_none(), "{}", String::from_utf8_lossy(bad));
    }
    Ok(())
}

#[tokio::test]
async fn the_question_names_the_request_the_way_the_protocol_says() -> TestResult {
    let (mut peer, decider) = open(DeciderOptions::default()).await?;
    let asked = Asked {
        query: Some("a=1&b"),
        sni: Some("api.github.com"),
        ..Asked::default()
    };
    let (_, req) = joined(decide(&decider, &asked), async {
        let req = peer.read().await?;
        peer.verdict(1, json!({"action": "allow"})).await?;
        Ok(req)
    })
    .await?;
    assert_eq!(
        req,
        json!({
            "t": "req", "id": 1, "method": "POST", "host": "api.github.com", "port": 443,
            "hostHeader": "api.github.com", "sni": "api.github.com", "path": "/repos",
            "query": "a=1&b", "headers": {"x-one": ["1"]}
        })
    );
    Ok(())
}

#[tokio::test]
async fn empty_optional_fields_are_left_out_of_the_question() -> TestResult {
    let (mut peer, decider) = open(DeciderOptions::default()).await?;
    let asked = Asked {
        headers: Vec::new(),
        ..Asked::default()
    };
    let (_, req) = joined(decide(&decider, &asked), async {
        let req = peer.read().await?;
        peer.verdict(1, json!({"action": "allow"})).await?;
        Ok(req)
    })
    .await?;
    let keys: Vec<&String> = req.as_object().ok_or("expected a value")?.keys().collect();
    for absent in ["hostHeader", "sni", "query", "headers"] {
        assert!(!keys.iter().any(|key| *key == absent), "{absent}");
    }
    Ok(())
}

#[tokio::test]
async fn request_ids_start_at_one_and_increase() -> TestResult {
    let (mut peer, decider) = open(DeciderOptions::default()).await?;
    let asked = Asked::default();
    for expected in 1..=3 {
        let (_, id) = joined(decide(&decider, &asked), async {
            let req = peer.read().await?;
            let id = req["id"].as_u64().ok_or("expected an id")?;
            peer.verdict(id, json!({"action": "allow"})).await?;
            Ok(id)
        })
        .await?;
        assert_eq!(id, expected);
    }
    Ok(())
}

#[tokio::test]
async fn an_allow_is_allowed_and_edits_headers() -> TestResult {
    assert_eq!(answered(json!({"action": "allow"})).await?, RequestDecision::Allow);
    let decision = answered(json!({
        "action": "allow", "setHeaders": {"x-new": ["a", "b"], "x-empty": []}, "removeHeaders": ["x-old"]
    }))
    .await?;
    assert!(matches!(decision, RequestDecision::AllowWith(_)));
    Ok(())
}

#[tokio::test]
async fn a_credential_is_placed_as_a_bearer_token_or_by_a_placement() -> TestResult {
    let credential = json!({"action": "allow", "cred": "github", "credential": "tok"});
    let plain = answered(credential.clone()).await?;
    let RequestDecision::AllowWith(allow) = plain else {
        return Err("expected an allow with edits".into());
    };
    assert!(format!("{allow:?}").contains("Bearer tok"));

    let (mut peer, decider) = open(
        DeciderOptions::default().credential_placements(vec![CredentialPlacement {
            host: Some("api.github.com".to_string()),
            ..CredentialPlacement::github()
        }]),
    )
    .await?;
    let asked = Asked::default();
    let (decision, ()) = joined(decide(&decider, &asked), async {
        peer.read().await?;
        peer.verdict(1, credential).await?;
        Ok(())
    })
    .await?;
    let RequestDecision::AllowWith(allow) = decision else {
        return Err("expected an allow with edits".into());
    };
    assert!(format!("{allow:?}").contains("Basic eC1hY2Nlc3MtdG9rZW46dG9r"));
    Ok(())
}

#[tokio::test]
async fn a_deny_keeps_its_status_and_reason() -> TestResult {
    let decision = answered(json!({"action": "deny", "status": 451, "reason": "no"})).await?;
    assert_eq!(status_of(&decision)?, (451, "no".to_string()));
    let RequestDecision::DenyWith(denial) = decision else {
        return Err("expected a denial".into());
    };
    let answer = crate::deny_header_style::DenyHeaderStyle::DenyReason
        .scope(async {
            denial.answer(
                crate::framed_head::AnsweredRequest::new("GET", false),
                std::time::SystemTime::now(),
            )
        })
        .await;
    assert!(String::from_utf8(answer)?.contains("\r\nX-Deny-Reason: decider\r\n"));
    let plain = answered(json!({"action": "deny", "status": 403})).await?;
    assert_eq!(status_of(&plain)?.0, 403);
    Ok(())
}

#[tokio::test]
async fn a_verdict_outside_the_protocol_kills_the_exchange_and_fails_closed() -> TestResult {
    let violations = [
        json!({"action": "deny", "status": 200}),
        json!({"action": "deny", "status": 600}),
        json!({"action": "deny", "status": 403, "reason": ""}),
        json!({"action": "deny", "status": 403, "reason": "a\nb"}),
        json!({"action": "deny", "status": 403, "extra": 1}),
        json!({"action": "allow", "setHeaders": {"authorization": ["x"]}}),
        json!({"action": "allow", "setHeaders": {"Mixed": ["x"]}}),
        json!({"action": "allow", "setHeaders": {}}),
        json!({"action": "allow", "setHeaders": {"x": ["a\r\nb"]}}),
        json!({"action": "allow", "removeHeaders": []}),
        json!({"action": "allow", "removeHeaders": ["host"]}),
        json!({"action": "allow", "removeHeaders": ["x", "x"]}),
        json!({"action": "allow", "cred": "github"}),
        json!({"action": "allow", "credential": "tok"}),
        json!({"action": "allow", "cred": "", "credential": "tok"}),
        json!({"action": "allow", "cred": "g", "credential": "has space"}),
        json!({"action": "need_body", "max": 0}),
        json!({"action": "need_body", "max": 33554433}),
        json!({"action": "other"}),
        json!({"action": "allow", "unknown": true}),
    ];
    for violation in violations {
        let decision = answered(violation.clone()).await?;
        assert_eq!(status_of(&decision)?.0, 503, "{violation}");
    }
    Ok(())
}

#[tokio::test]
async fn a_dead_exchange_refuses_every_later_request() -> TestResult {
    let (mut peer, decider) = open(DeciderOptions::default()).await?;
    let asked = Asked::default();
    let (first, ()) = joined(decide(&decider, &asked), async {
        peer.read().await?;
        peer.send_raw(b"not json").await?;
        Ok(())
    })
    .await?;
    assert_eq!(status_of(&first)?.0, 503);
    assert_eq!(decider.closed().await, "bad verdict frame");
    let second = decide(&decider, &asked).await?;
    assert_eq!(status_of(&second)?, (503, "request decider unavailable".to_string()));
    Ok(())
}

#[tokio::test]
async fn the_ways_an_exchange_ends_are_named() -> TestResult {
    let cases: [(&[u8], &str); 4] = [
        (
            br#"{"t":"verdict","id":1,"action":"allow","a":null}"#,
            "bad verdict frame",
        ),
        (
            br#"{"t":"verdict","id":9,"action":"allow"}"#,
            "verdict for id 9, which nothing waits on",
        ),
        (
            br#"{"t":"verdict","id":1,"action":"allow","t":"verdict"}"#,
            "bad verdict frame",
        ),
        (
            br#"{"t":"hello","proto":1,"allowedDomains":[],"deniedDomains":[]}"#,
            "bad verdict frame",
        ),
    ];
    for (raw, why) in cases {
        let (mut peer, decider) = open(DeciderOptions::default()).await?;
        peer.send_raw(raw).await?;
        assert_eq!(decider.closed().await, why);
    }
    Ok(())
}

#[tokio::test]
async fn an_oversized_frame_ends_the_exchange() -> TestResult {
    let (mut peer, decider) = open(DeciderOptions::default()).await?;
    peer.writer
        .write_all(&((MAX_READ_FRAME as u32) + 1).to_be_bytes())
        .await?;
    assert_eq!(decider.closed().await, "oversized frame");
    Ok(())
}

#[tokio::test]
async fn a_decider_that_hangs_up_ends_the_exchange() -> TestResult {
    let (peer, decider) = open(DeciderOptions::default()).await?;
    drop(peer);
    assert_eq!(decider.closed().await, "decider closed the stream");
    Ok(())
}

#[tokio::test]
async fn closing_the_decider_ends_the_exchange_once() -> TestResult {
    let (_peer, decider) = open(DeciderOptions::default()).await?;
    decider.close();
    assert_eq!(decider.closed().await, "closed");
    decider.inner.die("later");
    assert_eq!(decider.closed().await, "closed");
    let decision = decide(&decider, &Asked::default()).await?;
    assert_eq!(status_of(&decision)?.0, 503);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_slow_verdict_answers_503_and_its_late_verdict_is_dropped() -> TestResult {
    let (mut peer, decider) = open(DeciderOptions::default()).await?;
    let asked = Asked::default();
    let (decision, id) = joined(decide(&decider, &asked), async {
        Ok(peer.read().await?["id"].as_u64().ok_or("expected an id")?)
    })
    .await?;
    assert_eq!(
        status_of(&decision)?,
        (503, "request decider: decider timed out".to_string())
    );
    peer.verdict(id, json!({"action": "allow"})).await?;
    let (next, ()) = joined(decide(&decider, &asked), async {
        let req = peer.read().await?;
        peer.verdict(req["id"].as_u64().ok_or("expected an id")?, json!({"action": "allow"}))
            .await?;
        Ok(())
    })
    .await?;
    assert_eq!(next, RequestDecision::Allow);
    assert_eq!(decider.inner.state().dead, None);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_late_need_body_after_a_shown_body_kills_the_exchange() -> TestResult {
    let (mut peer, decider) = open(DeciderOptions::default()).await?;
    let asked = Asked {
        body: Some(b"abc"),
        ..Asked::default()
    };
    let (decision, id) = joined(decide(&decider, &asked), async {
        let id = peer.read().await?["id"].as_u64().ok_or("expected an id")?;
        peer.verdict(id, json!({"action": "need_body", "max": 10})).await?;
        assert_eq!(peer.read().await?["t"], "body");
        Ok(id)
    })
    .await?;
    assert_eq!(status_of(&decision)?.1, "request decider: decider timed out");
    peer.verdict(id, json!({"action": "need_body", "max": 10})).await?;
    assert_eq!(decider.closed().await, "need_body where none may come");
    Ok(())
}

#[tokio::test]
async fn need_body_shows_the_body_once_and_marks_a_cut() -> TestResult {
    for (max, data, cut) in [(10, "YWJjZGVm", false), (3, "YWJj", true)] {
        let (mut peer, decider) = open(DeciderOptions::default()).await?;
        let asked = Asked {
            body: Some(b"abcdef"),
            headers: vec![("Content-Length".to_string(), "6".to_string())],
            ..Asked::default()
        };
        let (decision, body) = joined(decide(&decider, &asked), async {
            peer.read().await?;
            peer.verdict(1, json!({"action": "need_body", "max": max})).await?;
            let body = peer.read().await?;
            peer.verdict(1, json!({"action": "allow"})).await?;
            Ok(body)
        })
        .await?;
        assert_eq!(decision, RequestDecision::Allow);
        let mut expected = json!({"t": "body", "id": 1, "data": data});
        if cut {
            expected["cut"] = json!(true);
        }
        assert_eq!(body, expected);
        assert_eq!(decider.body_bytes_held(), 0);
    }
    Ok(())
}

#[tokio::test]
async fn a_second_need_body_breaks_the_protocol() -> TestResult {
    let (mut peer, decider) = open(DeciderOptions::default()).await?;
    let asked = Asked {
        body: Some(b"abc"),
        ..Asked::default()
    };
    let (decision, ()) = joined(decide(&decider, &asked), async {
        peer.read().await?;
        peer.verdict(1, json!({"action": "need_body", "max": 10})).await?;
        peer.read().await?;
        peer.verdict(1, json!({"action": "need_body", "max": 10})).await?;
        Ok(())
    })
    .await?;
    assert_eq!(
        status_of(&decision)?,
        (503, "request decider: decider broke the protocol".to_string())
    );
    assert_eq!(decider.closed().await, "a second need_body");
    Ok(())
}

#[tokio::test]
async fn a_request_without_a_body_to_show_fails_closed() -> TestResult {
    let (mut peer, decider) = open(DeciderOptions::default()).await?;
    let asked = Asked::default();
    let (decision, ()) = joined(decide(&decider, &asked), async {
        peer.read().await?;
        peer.verdict(1, json!({"action": "need_body", "max": 10})).await?;
        Ok(())
    })
    .await?;
    assert_eq!(status_of(&decision)?.0, 503);
    Ok(())
}

#[tokio::test]
async fn a_body_for_a_method_that_has_none_is_refused_not_shown() -> TestResult {
    let (mut peer, decider) = open(DeciderOptions::default()).await?;
    let asked = Asked {
        method: "GET",
        body: Some(b"sneaky"),
        ..Asked::default()
    };
    let (decision, ()) = joined(decide(&decider, &asked), async {
        peer.read().await?;
        peer.verdict(1, json!({"action": "need_body", "max": 10})).await?;
        Ok(())
    })
    .await?;
    assert_eq!(status_of(&decision)?, (403, BODYLESS_METHOD_BODY_REASON.to_string()));
    peer.verdict(1, json!({"action": "deny", "status": 403})).await?;
    let next = Asked::default();
    let (allowed, ()) = joined(decide(&decider, &next), async {
        peer.read().await?;
        peer.verdict(2, json!({"action": "allow"})).await?;
        Ok(())
    })
    .await?;
    assert_eq!(allowed, RequestDecision::Allow);
    Ok(())
}

#[tokio::test]
async fn a_bodyless_method_with_no_body_is_shown_an_empty_one() -> TestResult {
    let (mut peer, decider) = open(DeciderOptions::default()).await?;
    let asked = Asked {
        method: "GET",
        body: Some(b""),
        ..Asked::default()
    };
    let (decision, body) = joined(decide(&decider, &asked), async {
        peer.read().await?;
        peer.verdict(1, json!({"action": "need_body", "max": 10})).await?;
        let body = peer.read().await?;
        peer.verdict(1, json!({"action": "allow"})).await?;
        Ok(body)
    })
    .await?;
    assert_eq!(decision, RequestDecision::Allow);
    assert_eq!(body, json!({"t": "body", "id": 1, "data": ""}));
    Ok(())
}

#[tokio::test]
async fn a_bodyless_method_that_declares_a_length_is_refused_before_any_byte_arrives() -> TestResult {
    let (mut peer, decider) = open(DeciderOptions::default()).await?;
    let asked = Asked {
        method: "GET",
        headers: vec![("Content-Length".to_string(), "5".to_string())],
        body: Some(b""),
        ..Asked::default()
    };
    let (decision, ()) = joined(decide(&decider, &asked), async {
        peer.read().await?;
        peer.verdict(1, json!({"action": "need_body", "max": 10})).await?;
        Ok(())
    })
    .await?;
    assert_eq!(status_of(&decision)?, (403, BODYLESS_METHOD_BODY_REASON.to_string()));
    Ok(())
}

#[tokio::test]
async fn bodies_held_for_inspection_are_capped_and_released() -> TestResult {
    let (mut peer, decider) = open(DeciderOptions::with_max_body_bytes_held(10)).await?;
    let asked = Asked {
        body: Some(b"0123456789abcdef"),
        headers: vec![("Content-Length".to_string(), "16".to_string())],
        ..Asked::default()
    };
    let (decision, ()) = joined(decide(&decider, &asked), async {
        peer.read().await?;
        peer.verdict(1, json!({"action": "need_body", "max": 100})).await?;
        Ok(())
    })
    .await?;
    assert_eq!(
        status_of(&decision)?,
        (
            503,
            "request decider: too many request bodies held for inspection".to_string()
        )
    );
    assert_eq!(decider.body_bytes_held(), 0);
    let fits = Asked {
        body: Some(b"0123456789"),
        headers: vec![("Content-Length".to_string(), "10".to_string())],
        ..Asked::default()
    };
    let (decision, ()) = joined(decide(&decider, &fits), async {
        peer.read().await?;
        peer.verdict(2, json!({"action": "need_body", "max": 100})).await?;
        peer.read().await?;
        peer.verdict(2, json!({"action": "allow"})).await?;
        Ok(())
    })
    .await?;
    assert_eq!(decision, RequestDecision::Allow);
    Ok(())
}

#[tokio::test]
async fn a_request_the_protocol_cannot_carry_answers_400_without_asking() -> TestResult {
    let (mut peer, decider) = open(DeciderOptions::default()).await?;
    let huge = "q".repeat(MAX_READ_FRAME);
    let cases = [
        (
            Asked {
                query: Some(&huge),
                ..Asked::default()
            },
            "request too large for the decider",
        ),
        (
            Asked {
                path: &"p".repeat(9000),
                ..Asked::default()
            },
            "request target too large",
        ),
        (
            Asked {
                headers: vec![("X".to_string(), "a\nb".to_string())],
                ..Asked::default()
            },
            "request header the decider cannot be asked about",
        ),
        (
            Asked {
                headers: vec![
                    ("Host".to_string(), "a".to_string()),
                    ("host".to_string(), "b".to_string()),
                ],
                ..Asked::default()
            },
            "more than one Host header",
        ),
    ];
    for (asked, reason) in cases {
        let decision = decide(&decider, &asked).await?;
        assert_eq!(status_of(&decision)?, (400, reason.to_string()));
    }
    let plain = Asked::default();
    let (decision, id) = joined(decide(&decider, &plain), async {
        let id = peer.read().await?["id"].as_u64().ok_or("expected an id")?;
        peer.verdict(id, json!({"action": "allow"})).await?;
        Ok(id)
    })
    .await?;
    assert_eq!(decision, RequestDecision::Allow);
    assert_eq!(id, 1);
    Ok(())
}

#[tokio::test]
async fn the_header_limit_is_kept_exactly() -> TestResult {
    let (mut peer, decider) = open(DeciderOptions::default()).await?;
    let pads = |count: usize| -> Vec<(String, String)> {
        (0..count).map(|i| (format!("X-Pad-{i}"), "v".to_string())).collect()
    };
    let at = Asked {
        headers: pads(190),
        ..Asked::default()
    };
    let (decision, ()) = joined(decide(&decider, &at), async {
        let id = peer.read().await?["id"].as_u64().ok_or("expected an id")?;
        peer.verdict(id, json!({"action": "allow"})).await?;
        Ok(())
    })
    .await?;
    assert_eq!(decision, RequestDecision::Allow);
    let past = Asked {
        headers: pads(191),
        ..Asked::default()
    };
    let decision = decide(&decider, &past).await?;
    assert_eq!(status_of(&decision)?, (400, "request headers too large".to_string()));
    let plain = Asked::default();
    let (decision, ()) = joined(decide(&decider, &plain), async {
        let id = peer.read().await?["id"].as_u64().ok_or("expected an id")?;
        peer.verdict(id, json!({"action": "allow"})).await?;
        Ok(())
    })
    .await?;
    assert_eq!(decision, RequestDecision::Allow);
    Ok(())
}

#[tokio::test]
async fn the_target_limit_is_kept_exactly() -> TestResult {
    let (mut peer, decider) = open(DeciderOptions::default()).await?;
    let at = format!("/{}", "a".repeat(crate::request_decider::MAX_HOST_PATH - 1));
    let asked = Asked {
        path: &at,
        ..Asked::default()
    };
    let (decision, ()) = joined(decide(&decider, &asked), async {
        let id = peer.read().await?["id"].as_u64().ok_or("expected an id")?;
        peer.verdict(id, json!({"action": "allow"})).await?;
        Ok(())
    })
    .await?;
    assert_eq!(decision, RequestDecision::Allow);
    let past = format!("/{}", "a".repeat(crate::request_decider::MAX_HOST_PATH));
    let asked = Asked {
        path: &past,
        ..Asked::default()
    };
    let decision = decide(&decider, &asked).await?;
    assert_eq!(status_of(&decision)?, (400, "request target too large".to_string()));
    Ok(())
}

#[tokio::test]
async fn a_request_with_many_header_lines_reaches_the_decider_whole() -> TestResult {
    let (mut peer, decider) = open(DeciderOptions::default()).await?;
    let asked = Asked {
        path: "/many",
        query: Some("q=1"),
        headers: (0..70).map(|i| (format!("X-Line-{i}"), format!("v{i}"))).collect(),
        ..Asked::default()
    };
    let (decision, frame) = joined(decide(&decider, &asked), async {
        let frame = peer.read().await?;
        let id = frame["id"].as_u64().ok_or("expected an id")?;
        peer.verdict(id, json!({"action": "allow"})).await?;
        Ok(frame)
    })
    .await?;
    assert_eq!(decision, RequestDecision::Allow);
    assert_eq!(frame["path"], "/many");
    assert_eq!(frame["query"], "q=1");
    for i in 0..70 {
        assert_eq!(frame["headers"][format!("x-line-{i}")], json!([format!("v{i}")]));
    }
    Ok(())
}

#[tokio::test]
async fn a_decider_that_does_not_read_ends_the_exchange() -> TestResult {
    let (_peer, decider) = open(DeciderOptions::default().max_queued_bytes(64)).await?;
    let decision = decide(&decider, &Asked::default()).await?;
    assert_eq!(status_of(&decision)?.0, 503);
    assert!(decider.closed().await.starts_with("not reading: "));
    Ok(())
}

async fn exchange(frame: Vec<u8>) -> Result<(RequestDecision, Option<String>), TestError> {
    let (mut peer, decider) = open(DeciderOptions::default()).await?;
    let asked = Asked::default();
    let (decision, ()) = joined(decide(&decider, &asked), async {
        peer.read().await?;
        peer.send_raw(&frame).await?;
        Ok(())
    })
    .await?;
    let dead = tokio::time::timeout(Duration::from_millis(50), decider.closed())
        .await
        .ok();
    Ok((decision, dead))
}

fn verdict_frame(rest: Value) -> Result<Vec<u8>, TestError> {
    let mut message = json!({"t": "verdict", "id": 1});
    object_of_mut(&mut message)?.extend(object_of(&rest)?.clone());
    Ok(serde_json::to_vec(&message)?)
}

fn held_by_40() -> (Asked<'static>, DeciderOptions) {
    (
        Asked {
            body: Some(&[b'x'; 40]),
            headers: vec![("Content-Length".to_string(), "40".to_string())],
            ..Asked::default()
        },
        DeciderOptions::with_max_body_bytes_held(40).timeout(Duration::from_millis(200)),
    )
}

#[tokio::test]
async fn held_body_bytes_are_given_back_after_a_deny_an_abort_a_timeout_and_a_dead_decider() -> TestResult {
    let (asked, options) = held_by_40();
    let (mut peer, decider) = open(options).await?;

    let (denied, ()) = joined(decide(&decider, &asked), async {
        peer.read().await?;
        peer.verdict(1, json!({"action": "need_body", "max": 40})).await?;
        peer.read().await?;
        assert_eq!(decider.body_bytes_held(), 40);
        peer.verdict(1, json!({"action": "deny", "status": 403, "reason": "no"}))
            .await?;
        Ok(())
    })
    .await?;
    assert_eq!(status_of(&denied)?, (403, "no".to_string()));
    assert_eq!(decider.body_bytes_held(), 0);

    let (source, abort) = Gated::new();
    let streamed = Asked {
        source: Some(&source),
        ..Asked::default()
    };
    let (aborted, ()) = joined(decide(&decider, &streamed), async {
        peer.read().await?;
        peer.verdict(2, json!({"action": "need_body", "max": 40})).await?;
        until(|| decider.body_bytes_held() == 40).await;
        if abort.send(Err(BodyUnavailableError::Unreadable)).is_err() {
            return Err("the body reader is gone".into());
        }
        Ok(())
    })
    .await?;
    assert_eq!(status_of(&aborted)?.0, 503);
    assert_eq!(decider.body_bytes_held(), 0);

    let (timed_out, ()) = joined(decide(&decider, &asked), async {
        peer.read().await?;
        peer.verdict(3, json!({"action": "need_body", "max": 40})).await?;
        peer.read().await?;
        Ok(())
    })
    .await?;
    assert_eq!(status_of(&timed_out)?.0, 503);
    assert_eq!(decider.body_bytes_held(), 0);

    let (dead, ()) = joined(decide(&decider, &asked), async {
        peer.read().await?;
        peer.verdict(4, json!({"action": "need_body", "max": 40})).await?;
        peer.read().await?;
        decider.close();
        Ok(())
    })
    .await?;
    assert_eq!(status_of(&dead)?.0, 503);
    assert_eq!(decider.body_bytes_held(), 0);
    assert_eq!(decider.closed().await, "closed");
    Ok(())
}

#[tokio::test]
async fn a_deny_while_the_body_is_being_read_is_a_deny_and_no_body_is_sent() -> TestResult {
    let (_, options) = held_by_40();
    let (mut peer, decider) = open(options.timeout(Duration::from_secs(5))).await?;
    let (source, _keeps_the_body_open) = Gated::new();
    let streamed = Asked {
        source: Some(&source),
        ..Asked::default()
    };
    let (decision, ()) = joined(decide(&decider, &streamed), async {
        peer.read().await?;
        peer.verdict(1, json!({"action": "need_body", "max": 40})).await?;
        until(|| decider.body_bytes_held() == 40).await;
        peer.verdict(1, json!({"action": "deny", "status": 403, "reason": "no"}))
            .await?;
        Ok(())
    })
    .await?;
    assert_eq!(status_of(&decision)?, (403, "no".to_string()));
    assert!(
        tokio::time::timeout(Duration::from_millis(50), peer.read())
            .await
            .is_err()
    );
    assert_eq!(decider.body_bytes_held(), 0);
    let plain = Asked::default();
    let (next, ()) = joined(decide(&decider, &plain), async {
        peer.read().await?;
        peer.verdict(2, json!({"action": "allow"})).await?;
        Ok(())
    })
    .await?;
    assert_eq!(next, RequestDecision::Allow);
    Ok(())
}

#[tokio::test]
async fn need_body_on_a_body_still_arriving_is_cut_at_max_and_decided_at_once() -> TestResult {
    let (mut peer, decider) = open(DeciderOptions::default()).await?;
    let (mut client, mut from_client) = duplex(64 << 10);
    let sending = tokio::spawn(async move {
        for _ in 0..32 {
            if client.write_all(&[b'x'; 64 << 10]).await.is_err() {
                return;
            }
        }
        std::future::pending::<()>().await;
    });
    let mut buffered = Vec::new();
    let continued = AtomicBool::new(false);
    let source = StreamedBody::new(
        &mut from_client,
        &mut buffered,
        Framing::ContentLength(2 << 20),
        false,
        &continued,
    );
    let streamed = Asked {
        headers: vec![("Content-Length".to_string(), (2 << 20).to_string())],
        source: Some(&source),
        ..Asked::default()
    };
    let decided = tokio::time::timeout(Duration::from_secs(10), async {
        joined(decide(&decider, &streamed), async {
            peer.read().await?;
            peer.verdict(1, json!({"action": "need_body", "max": 1 << 20})).await?;
            let body = peer.read_unbounded().await?;
            peer.verdict(1, json!({"action": "allow"})).await?;
            Ok(body)
        })
        .await
    })
    .await;
    sending.abort();
    let (decision, body) = decided??;
    assert_eq!(decision, RequestDecision::Allow);
    assert_eq!(body["cut"], json!(true));
    let shown = STANDARD.decode(body["data"].as_str().ok_or("expected a value")?)?;
    assert_eq!(shown.len(), 1 << 20);
    assert_eq!(decider.body_bytes_held(), 0);
    Ok(())
}

#[tokio::test]
async fn verdicts_in_another_spelling_or_type_kill_the_exchange() -> TestResult {
    let cases = [
        verdict_frame(json!({"action": "allow", "setHeaders": {"x_api_key": ["k"]}}))?,
        verdict_frame(json!({"action": "allow", "setHeaders": {"x-api-key": ["k"]}}))?,
        br#"{"t":"verdict","id":1,"action":"deny","status":"403"}"#.to_vec(),
        verdict_frame(json!({"action": "deny", "status": 403, "reason": "a\u{85}b"}))?,
    ];
    for frame in cases {
        let (decision, dead) = exchange(frame.clone()).await?;
        let shown = String::from_utf8_lossy(&frame).to_string();
        assert_eq!(status_of(&decision)?.0, 503, "{shown}");
        assert_eq!(dead.as_deref(), Some("bad verdict frame"), "{shown}");
    }
    Ok(())
}

#[tokio::test]
async fn a_credential_is_one_to_8192_printable_ascii_taken_as_is() -> TestResult {
    let bad = [
        String::new(),
        "a b".to_string(),
        "a\tb".to_string(),
        "ab\r\nX-Evil: 1".to_string(),
        "a\u{85}b".to_string(),
        "\u{e9}".to_string(),
        "x".repeat(8193),
    ];
    for credential in bad {
        let frame = verdict_frame(json!({"action": "allow", "cred": "session-token", "credential": credential}))?;
        let (decision, dead) = exchange(frame).await?;
        assert_eq!(status_of(&decision)?.0, 503, "{credential:?}");
        assert!(dead.is_some(), "{credential:?}");
    }
    for credential in ["x".to_string(), "~".repeat(8192)] {
        let frame = verdict_frame(json!({"action": "allow", "cred": "session-token", "credential": credential}))?;
        let (decision, dead) = exchange(frame).await?;
        let RequestDecision::AllowWith(allow) = decision else {
            return Err("expected an allow with edits".into());
        };
        assert!(format!("{allow:?}").contains(&format!("Bearer {credential}")));
        assert_eq!(dead, None);
    }
    Ok(())
}

#[tokio::test]
async fn a_placed_credential_removes_every_credential_header_first() -> TestResult {
    let decision = answered(json!({"action": "allow", "cred": "session-token", "credential": "tok"})).await?;
    let RequestDecision::AllowWith(allow) = decision else {
        return Err("expected an allow with edits".into());
    };
    let shown = format!("{allow:?}");
    for name in CREDENTIAL_HEADERS {
        assert!(shown.contains(&format!("\"{name}\"")), "{name}");
    }
    Ok(())
}

#[tokio::test]
async fn an_upper_case_domain_in_the_hello_ends_the_exchange() -> TestResult {
    for hello in [
        json!({"t": "hello", "proto": 1, "allowedDomains": ["Example.test"], "deniedDomains": []}),
        json!({"t": "hello", "proto": 1, "allowedDomains": [], "deniedDomains": ["Example.test"]}),
    ] {
        let (_, opened) = open_with(DeciderOptions::default(), hello.clone()).await?;
        assert_eq!(
            opened.err().as_deref(),
            Some("decider unavailable: bad hello"),
            "{hello}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn a_900_kib_query_is_sent_to_the_decider() -> TestResult {
    let (mut peer, decider) = open(DeciderOptions::default()).await?;
    let query = "q".repeat(900 * 1024);
    let asked = Asked {
        query: Some(&query),
        ..Asked::default()
    };
    let (decision, ()) = joined(decide(&decider, &asked), async {
        let req = peer.read().await?;
        assert_eq!(req["query"].as_str().map(str::len), Some(900 * 1024));
        peer.verdict(1, json!({"action": "allow"})).await?;
        Ok(())
    })
    .await?;
    assert_eq!(decision, RequestDecision::Allow);
    Ok(())
}

#[tokio::test]
async fn verdicts_carry_at_most_190_removals_and_190_set_values() -> TestResult {
    let names = |n: usize| -> Vec<String> { (0..n).map(|i| format!("x-h-{i}")).collect() };
    let values = |n: usize| -> Vec<String> { (0..n).map(|i| format!("v{i}")).collect() };
    for (label, rest, fine) in [
        (
            "190 removals",
            json!({"action": "allow", "removeHeaders": names(190)}),
            true,
        ),
        (
            "191 removals",
            json!({"action": "allow", "removeHeaders": names(191)}),
            false,
        ),
        (
            "190 set values",
            json!({"action": "allow", "setHeaders": {"x-s": values(190)}}),
            true,
        ),
        (
            "191 set values",
            json!({"action": "allow", "setHeaders": {"x-s": values(191)}}),
            false,
        ),
    ] {
        let (decision, dead) = exchange(verdict_frame(rest)?).await?;
        if fine {
            assert!(matches!(decision, RequestDecision::AllowWith(_)), "{label}");
            assert_eq!(dead, None, "{label}");
        } else {
            assert_eq!(status_of(&decision)?.0, 503, "{label}");
            assert!(dead.is_some(), "{label}");
        }
    }
    Ok(())
}

fn value(text: &str) -> Result<Value, TestError> {
    parse_strict(text.as_bytes()).ok_or_else(|| "expected an object".into())
}

#[test]
fn strict_json_takes_negative_and_fractional_numbers_and_refuses_what_it_cannot_hold() -> TestResult {
    assert!(parse_strict(br#"{"a":-1,"b":1.5}"#).is_some());
    let bytes = BytesDeserializer::<ValueError>::new(b"x");
    assert!(Strict::deserialize(bytes).is_err());
    let not_a_number = IntoDeserializer::<ValueError>::into_deserializer(f64::NAN);
    assert!(Strict::deserialize(not_a_number).is_err());
    Ok(())
}

#[test]
fn an_allow_with_an_unusable_id_is_not_a_verdict() -> TestResult {
    assert!(verdict_of(value(r#"{"t":"verdict","id":0,"action":"allow"}"#)?).is_err());
    assert!(verdict_of(value(r#"{"t":"other","id":1,"action":"allow"}"#)?).is_err());
    Ok(())
}

struct FailingReader;

impl AsyncRead for FailingReader {
    fn poll_read(self: Pin<&mut Self>, _cx: &mut Context<'_>, _buf: &mut ReadBuf<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Err(std::io::Error::other("the stream broke")))
    }
}

#[tokio::test]
async fn a_read_that_fails_names_the_read_as_the_reason() -> TestResult {
    let result = read_frame(&mut FailingReader).await;
    assert!(matches!(result, Err(why) if why.starts_with("read error")));
    Ok(())
}

#[tokio::test]
async fn a_decider_that_stops_reading_ends_the_exchange_with_the_write_error() -> TestResult {
    let (peer, decider) = open(DeciderOptions::default()).await?;
    let Peer {
        reader,
        writer: _writer,
    } = peer;
    drop(reader);
    let decision = decide(&decider, &Asked::default()).await?;
    assert_eq!(status_of(&decision)?.0, 503);
    assert!(decider.closed().await.starts_with("write error"));
    Ok(())
}

#[tokio::test]
async fn nothing_is_queued_once_the_exchange_has_ended() -> TestResult {
    let (_peer, decider) = open(DeciderOptions::default()).await?;
    decider.close();
    let before = decider.inner.state().queued;
    decider.inner.send(vec![1, 2, 3]);
    assert_eq!(decider.inner.state().queued, before);
    Ok(())
}

#[tokio::test]
async fn a_verdict_for_an_id_long_since_forgotten_is_ignored() -> TestResult {
    let (mut peer, decider) = open(DeciderOptions::default()).await?;
    decider.inner.state().last_id = 5000;
    peer.verdict(1, json!({"action": "allow"})).await?;
    let asked = Asked::default();
    let (decision, ()) = joined(decide(&decider, &asked), async {
        peer.read().await?;
        peer.verdict(5001, json!({"action": "allow"})).await?;
        Ok(())
    })
    .await?;
    assert_eq!(decision, RequestDecision::Allow);
    Ok(())
}

#[tokio::test]
async fn running_out_of_request_ids_ends_the_exchange() -> TestResult {
    let (_peer, decider) = open(DeciderOptions::default()).await?;
    decider.inner.state().last_id = MAX_ID;
    let decision = decide(&decider, &Asked::default()).await?;
    assert_eq!(status_of(&decision)?.0, 503);
    assert_eq!(decider.closed().await, "request ids exhausted");
    Ok(())
}

#[tokio::test]
async fn registering_on_an_ended_exchange_hands_back_the_reason() -> TestResult {
    let (_peer, decider) = open(DeciderOptions::default()).await?;
    decider.close();
    let mut rx = decider.register(3);
    assert!(matches!(rx.try_recv(), Ok(Event::Dead(why)) if why == "closed"));
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_verdict_taken_but_never_delivered_reads_as_an_ended_exchange() -> TestResult {
    let (_peer, decider) = open(DeciderOptions::default().timeout(Duration::from_secs(1))).await?;
    let mut rx = decider.register(7);
    let undelivered = decider.inner.state().waiters.remove(&7);
    assert!(undelivered.is_some());
    let waited = decider.wait(7, &mut rx, Late::Verdict).await;
    assert!(matches!(waited, Wait::Dead(why) if why.is_empty()));
    Ok(())
}

#[tokio::test]
async fn a_need_body_verdict_is_not_decided_as_an_allow() -> TestResult {
    let (_peer, decider) = open(DeciderOptions::default()).await?;
    let decision = decider.decision(Verdict::NeedBody { max: 5 }, "api.github.com");
    assert_eq!(status_of(&decision)?.0, 503);
    Ok(())
}

#[tokio::test]
async fn a_bodyless_method_whose_body_cannot_be_read_fails_closed() -> TestResult {
    let (mut peer, decider) = open(DeciderOptions::default()).await?;
    let (source, gate) = Gated::new();
    drop(gate);
    let asked = Asked {
        method: "GET",
        source: Some(&source),
        ..Asked::default()
    };
    let (decision, ()) = joined(decide(&decider, &asked), async {
        peer.read().await?;
        peer.verdict(1, json!({"action": "need_body", "max": 10})).await?;
        Ok(())
    })
    .await?;
    assert_eq!(status_of(&decision)?.0, 503);
    Ok(())
}

#[tokio::test]
async fn an_answer_that_comes_before_the_body_is_checked_ends_the_exchange() -> TestResult {
    let (mut peer, decider) = open(DeciderOptions::default()).await?;
    let (source, gate) = Gated::new();
    let asked = Asked {
        method: "GET",
        source: Some(&source),
        ..Asked::default()
    };
    let (decision, ()) = joined(decide(&decider, &asked), async {
        peer.read().await?;
        peer.verdict(1, json!({"action": "need_body", "max": 10})).await?;
        until(|| decider.inner.state().late.contains_key(&1)).await;
        peer.verdict(1, json!({"action": "allow"})).await?;
        until(|| !decider.inner.state().late.contains_key(&1)).await;
        assert!(
            gate.send(Ok(BodyRead {
                bytes: Vec::new(),
                cut: false
            }))
            .is_ok()
        );
        Ok(())
    })
    .await?;
    assert_eq!(status_of(&decision)?.0, 503);
    assert_eq!(decider.closed().await, "answered before it was sent the body");
    Ok(())
}

enum Rig {
    End,
    Strand,
    Say(Event),
}

struct Rigged {
    inner: Arc<Inner>,
    id: u64,
    rig: Mutex<Option<Rig>>,
    kept: Mutex<Vec<oneshot::Sender<Event>>>,
}

impl Rigged {
    fn new(decider: &FramedDecider, rig: Rig) -> Self {
        Self {
            inner: Arc::clone(&decider.inner),
            id: 1,
            rig: Mutex::new(Some(rig)),
            kept: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl BodySource for Rigged {
    async fn read(&self, _max: usize) -> BodyOutcome {
        let rig = self.rig.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).take();
        match rig {
            Some(Rig::End) => self.inner.die("ended while the body was read"),
            Some(Rig::Strand) => {
                let waiter = self.inner.state().waiters.remove(&self.id);
                self.kept
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .extend(waiter);
            }
            Some(Rig::Say(event)) => {
                let waiter = self.inner.state().waiters.remove(&self.id);
                if let Some(waiter) = waiter {
                    let _ = waiter.send(event);
                }
            }
            None => {}
        }
        Ok(BodyRead {
            bytes: Vec::new(),
            cut: false,
        })
    }
}

async fn read_while_rigged(rig: impl FnOnce() -> Rig) -> Result<(RequestDecision, FramedDecider), TestError> {
    let (mut peer, decider) = open(DeciderOptions::default()).await?;
    let source = Rigged::new(&decider, rig());
    let asked = Asked {
        source: Some(&source),
        ..Asked::default()
    };
    let (decision, ()) = joined(decide(&decider, &asked), async {
        peer.read().await?;
        peer.verdict(1, json!({"action": "need_body", "max": 10})).await?;
        Ok(())
    })
    .await?;
    Ok((decision, decider))
}

#[tokio::test]
async fn an_exchange_that_ends_as_the_body_arrives_fails_closed() -> TestResult {
    let (decision, decider) = read_while_rigged(|| Rig::End).await?;
    assert_eq!(status_of(&decision)?.0, 503);
    assert_eq!(decider.closed().await, "ended while the body was read");
    Ok(())
}

#[tokio::test]
async fn a_body_that_arrives_after_the_waiter_vanished_fails_closed() -> TestResult {
    let (decision, _decider) = read_while_rigged(|| Rig::Strand).await?;
    assert_eq!(status_of(&decision)?.0, 503);
    Ok(())
}

#[tokio::test]
async fn a_non_deny_verdict_found_after_the_body_was_read_breaks_the_protocol() -> TestResult {
    let (decision, decider) = read_while_rigged(|| {
        Rig::Say(Event::Verdict(Verdict::Allow {
            set_headers: BTreeMap::new(),
            remove_headers: Vec::new(),
            cred: None,
            credential: None,
        }))
    })
    .await?;
    assert_eq!(status_of(&decision)?.0, 503);
    assert_eq!(
        decider.closed().await,
        "a verdict other than deny while the body was read"
    );
    Ok(())
}

#[tokio::test]
async fn an_ended_exchange_found_after_the_body_was_read_names_its_reason() -> TestResult {
    let (decision, _decider) = read_while_rigged(|| Rig::Say(Event::Dead("gone".to_string()))).await?;
    assert_eq!(status_of(&decision)?, (503, "request decider: gone".to_string()));
    Ok(())
}
