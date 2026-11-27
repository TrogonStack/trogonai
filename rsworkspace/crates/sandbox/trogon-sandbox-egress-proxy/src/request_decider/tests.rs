use std::sync::Mutex;

use trogon_sandbox_contract::Host;
use trogon_sandbox_contract::HttpMethod;

use super::*;
use crate::decider::DeciderTransportError;
use crate::request_body::RequestBody;
use crate::request_filter::RequestScheme;

struct Scripted {
    replies: Mutex<Vec<Result<Vec<u8>, DeciderTransportError>>>,
    seen: Mutex<Vec<RequestDecideRequest>>,
}

impl Scripted {
    fn new(replies: Vec<Result<&str, DeciderTransportError>>) -> Arc<Self> {
        Arc::new(Self {
            replies: Mutex::new(
                replies
                    .into_iter()
                    .rev()
                    .map(|reply| reply.map(|text| text.as_bytes().to_vec()))
                    .collect(),
            ),
            seen: Mutex::new(Vec::new()),
        })
    }

    fn seen(&self) -> Result<Vec<RequestDecideRequest>, Box<dyn std::error::Error>> {
        Ok(self.seen.lock().map_err(|_| "seen poisoned")?.clone())
    }
}

#[async_trait]
impl DeciderTransport for Scripted {
    async fn request(&self, subject: String, payload: Vec<u8>) -> Result<Vec<u8>, DeciderTransportError> {
        assert_eq!(subject, "sandbox.egress.request.lease-1");
        let question =
            serde_json::from_slice(&payload).map_err(|error| DeciderTransportError::Unavailable(error.to_string()))?;
        self.seen
            .lock()
            .map_err(|_| DeciderTransportError::Unavailable("seen poisoned".to_string()))?
            .push(question);
        self.replies
            .lock()
            .map_err(|_| DeciderTransportError::Unavailable("replies poisoned".to_string()))?
            .pop()
            .unwrap_or_else(|| Err(DeciderTransportError::Unavailable("no scripted reply".to_string())))
    }
}

fn decider(transport: Arc<Scripted>) -> RequestDecider {
    RequestDecider::new(LeaseId::new("lease-1".to_string()), transport, DeciderLimits::default())
}

async fn decide(
    decider: &RequestDecider,
    headers: &[(String, String)],
    body: Option<&[u8]>,
) -> Result<RequestDecision, Box<dyn std::error::Error>> {
    let host = Host::new("api.github.com")?;
    let method = HttpMethod::new("POST")?;
    Ok(decider
        .decide(&RequestView {
            host: &host,
            port: 443,
            method: &method,
            path: "/repos",
            query: None,
            sni: None,
            headers,
            body: body.map(|body| RequestBody::new(body, false)),
            scheme: RequestScheme::Https,
            forms: None,
        })
        .await)
}

fn headers() -> Vec<(String, String)> {
    vec![
        ("Host".to_string(), "api.github.com".to_string()),
        ("Cookie".to_string(), "a=b".to_string()),
        ("X-One".to_string(), "1".to_string()),
    ]
}

fn status_of(decision: &RequestDecision) -> Result<(u16, String), Box<dyn std::error::Error>> {
    let RequestDecision::DenyWith(denial) = decision else {
        return Err(format!("expected a denial, got {decision:?}").into());
    };
    Ok((denial.answer_status(), denial.denial_reason().to_string()))
}

#[tokio::test]
async fn the_question_carries_the_host_header_apart_and_lowercase_headers() -> Result<(), Box<dyn std::error::Error>> {
    let transport = Scripted::new(vec![Ok(r#"{"verdict":"allow"}"#)]);
    let decision = decide(&decider(transport.clone()), &headers(), None).await?;
    assert_eq!(decision, RequestDecision::Allow);
    let seen = transport.seen()?;
    assert_eq!(seen[0].host_header.as_deref(), Some("api.github.com"));
    assert_eq!(seen[0].headers["x-one"], vec!["1".to_string()]);
    assert!(!seen[0].headers.contains_key("host"));
    Ok(())
}

#[tokio::test]
async fn the_question_carries_the_sni_and_the_query_apart_from_the_path() -> Result<(), Box<dyn std::error::Error>> {
    let transport = Scripted::new(vec![Ok(r#"{"verdict":"allow"}"#)]);
    let host = Host::new("api.github.com")?;
    let method = HttpMethod::new("GET")?;
    let decision = decider(transport.clone())
        .decide(&RequestView {
            host: &host,
            port: 443,
            method: &method,
            path: "/repos",
            query: Some("a=1&b=2"),
            sni: Some("api.github.com"),
            headers: &[],
            body: None,
            scheme: RequestScheme::Https,
            forms: None,
        })
        .await;
    assert_eq!(decision, RequestDecision::Allow);
    let seen = transport.seen()?;
    assert_eq!(seen[0].path, "/repos");
    assert_eq!(seen[0].query.as_deref(), Some("a=1&b=2"));
    assert_eq!(seen[0].sni.as_deref(), Some("api.github.com"));
    Ok(())
}

#[tokio::test]
async fn an_empty_query_and_a_missing_sni_are_left_out_of_the_question() -> Result<(), Box<dyn std::error::Error>> {
    let transport = Scripted::new(vec![Ok(r#"{"verdict":"allow"}"#)]);
    let host = Host::new("api.github.com")?;
    let method = HttpMethod::new("GET")?;
    decider(transport.clone())
        .decide(&RequestView {
            host: &host,
            port: 80,
            method: &method,
            path: "/repos",
            query: Some(""),
            sni: None,
            headers: &[],
            body: None,
            scheme: RequestScheme::Https,
            forms: None,
        })
        .await;
    let seen = transport.seen()?;
    assert_eq!(seen[0].query, None);
    assert_eq!(seen[0].sni, None);
    Ok(())
}

#[tokio::test]
async fn an_allow_edits_headers_and_places_a_bearer_credential() -> Result<(), Box<dyn std::error::Error>> {
    let transport = Scripted::new(vec![Ok(
        r#"{"verdict":"allow","set_headers":{"x-added":["1","2"]},"remove_headers":["x-one"],"cred":"api","credential":"tok123"}"#,
    )]);
    let RequestDecision::AllowWith(allow) = decide(&decider(transport), &headers(), None).await? else {
        return Err("expected edits".into());
    };
    let mut edited = headers();
    allow.header_edits().apply(&mut edited);
    assert!(edited.contains(&("authorization".to_string(), "Bearer tok123".to_string())));
    assert!(edited.contains(&("x-added".to_string(), "1".to_string())));
    assert!(edited.contains(&("x-added".to_string(), "2".to_string())));
    assert!(!edited.iter().any(|(name, _)| name.eq_ignore_ascii_case("cookie")));
    assert!(!edited.iter().any(|(name, _)| name.eq_ignore_ascii_case("x-one")));
    Ok(())
}

#[tokio::test]
async fn a_placement_picks_basic_for_its_class_and_host() -> Result<(), Box<dyn std::error::Error>> {
    let transport = Scripted::new(vec![Ok(r#"{"verdict":"allow","cred":"github","credential":"tok"}"#)]);
    let decider = decider(transport).credential_placements(vec![CredentialPlacement {
        class: Some("github".to_string()),
        host: None,
        scheme: CredentialScheme::Basic {
            user: "x-access-token".to_string(),
        },
    }]);
    let RequestDecision::AllowWith(allow) = decide(&decider, &headers(), None).await? else {
        return Err("expected edits".into());
    };
    let mut edited = Vec::new();
    allow.header_edits().apply(&mut edited);
    let expected = format!("Basic {}", STANDARD.encode("x-access-token:tok"));
    assert_eq!(edited, vec![("authorization".to_string(), expected)]);
    Ok(())
}

#[tokio::test]
async fn a_deny_keeps_its_status_and_reason_and_is_marked() -> Result<(), Box<dyn std::error::Error>> {
    let transport = Scripted::new(vec![Ok(r#"{"verdict":"deny","status":451,"reason":"not here"}"#)]);
    let decision = decide(&decider(transport), &headers(), None).await?;
    assert_eq!(status_of(&decision)?, (451, "not here".to_string()));
    Ok(())
}

#[tokio::test]
async fn verdicts_that_break_the_protocol_fail_closed() -> Result<(), Box<dyn std::error::Error>> {
    for reply in [
        r#"{"verdict":"allow","set_headers":{"authorization":["x"]}}"#,
        r#"{"verdict":"allow","set_headers":{"Mixed":["x"]}}"#,
        r#"{"verdict":"allow","remove_headers":["host"]}"#,
        r#"{"verdict":"allow","cred":"api"}"#,
        r#"{"verdict":"allow","cred":"api","credential":"has space"}"#,
        r#"{"verdict":"deny","status":200}"#,
        r#"{"verdict":"nonsense"}"#,
        r#"{"verdict":"allow","extra":1}"#,
        "not json",
    ] {
        let transport = Scripted::new(vec![Ok(reply)]);
        let decision = decide(&decider(transport), &headers(), None).await?;
        assert_eq!(status_of(&decision)?.0, 503, "{reply}");
    }
    Ok(())
}

#[tokio::test]
async fn an_unreachable_or_slow_decider_answers_503() -> Result<(), Box<dyn std::error::Error>> {
    let transport = Scripted::new(vec![Err(DeciderTransportError::TimedOut)]);
    let decision = decide(&decider(transport), &headers(), None).await?;
    assert_eq!(status_of(&decision)?.0, 503);
    Ok(())
}

#[tokio::test]
async fn a_request_the_decider_cannot_be_asked_about_answers_400() -> Result<(), Box<dyn std::error::Error>> {
    let transport = Scripted::new(vec![]);
    let two_hosts = vec![
        ("Host".to_string(), "a".to_string()),
        ("Host".to_string(), "b".to_string()),
    ];
    let decision = decide(&decider(transport.clone()), &two_hosts, None).await?;
    assert_eq!(status_of(&decision)?.0, 400);
    let many: Vec<_> = (0..191).map(|n| (format!("x-{n}"), "v".to_string())).collect();
    let decision = decide(&decider(transport.clone()), &many, None).await?;
    assert_eq!(status_of(&decision)?.0, 400);
    assert!(transport.seen()?.is_empty());
    Ok(())
}

#[tokio::test]
async fn need_body_shows_the_body_once_and_marks_a_cut() -> Result<(), Box<dyn std::error::Error>> {
    let transport = Scripted::new(vec![
        Ok(r#"{"verdict":"need_body","max":4}"#),
        Ok(r#"{"verdict":"allow"}"#),
    ]);
    let decision = decide(&decider(transport.clone()), &headers(), Some(b"abcdef")).await?;
    assert_eq!(decision, RequestDecision::Allow);
    let seen = transport.seen()?;
    let body = seen[1].body.clone().ok_or("expected a value")?;
    assert_eq!(STANDARD.decode(body.data.as_str())?, b"abcd");
    assert!(bool::from(body.cut));
    assert!(seen[0].body.is_none());
    Ok(())
}

#[tokio::test]
async fn a_second_need_body_or_no_body_to_show_fails_closed() -> Result<(), Box<dyn std::error::Error>> {
    let transport = Scripted::new(vec![
        Ok(r#"{"verdict":"need_body","max":4}"#),
        Ok(r#"{"verdict":"need_body","max":4}"#),
    ]);
    let decision = decide(&decider(transport), &headers(), Some(b"ab")).await?;
    assert_eq!(status_of(&decision)?.0, 503);

    let transport = Scripted::new(vec![Ok(r#"{"verdict":"need_body","max":4}"#)]);
    let decision = decide(&decider(transport), &headers(), None).await?;
    assert_eq!(status_of(&decision)?.0, 503);

    let transport = Scripted::new(vec![Ok(r#"{"verdict":"need_body","max":33554433}"#)]);
    let decision = decide(&decider(transport), &headers(), Some(b"ab")).await?;
    assert_eq!(status_of(&decision)?.0, 503);
    Ok(())
}

fn framed(path: &str, extra: &[(&str, String)]) -> Result<RequestDecideRequest, String> {
    let host = Host::new("api.example.test").map_err(|error| error.to_string())?;
    let method = HttpMethod::new("GET").map_err(|error| error.to_string())?;
    let mut headers = vec![("Host".to_string(), "api.example.test".to_string())];
    headers.extend(extra.iter().map(|(n, v)| (n.to_string(), v.clone())));
    DeciderRules::with_placements(Vec::new())
        .question(
            &LeaseId::new(String::new()),
            &RequestView {
                host: &host,
                port: 443,
                method: &method,
                path,
                query: None,
                sni: None,
                headers: &headers,
                body: None,
                scheme: RequestScheme::Https,
                forms: None,
            },
        )
        .map_err(str::to_string)
}

#[test]
fn a_path_of_8_kib_in_bytes_passes_and_one_byte_more_does_not() -> Result<(), Box<dyn std::error::Error>> {
    let fits = format!("/{}a", "\u{e9}".repeat(4095));
    assert!(framed(&fits, &[]).is_ok());
    let over = format!("/{}", "\u{e9}".repeat(4096));
    assert_eq!(framed(&over, &[]).err().as_deref(), Some("request target too large"));
    Ok(())
}

#[test]
fn header_bytes_count_each_name_once() -> Result<(), Box<dyn std::error::Error>> {
    let big = "\u{e9}".repeat(32767);
    assert_eq!(
        framed("/", &[("x-a", big.clone())]).err().as_deref(),
        Some("request headers too large")
    );
    assert!(framed("/", &[("x", big)]).is_ok());
    let name = "n".repeat(2000);
    let many: Vec<(&str, String)> = (0..40).map(|_| (name.as_str(), "v".repeat(10))).collect();
    assert!(framed("/", &many).is_ok());
    Ok(())
}

#[test]
fn header_values_are_at_most_190_apart_from_host() -> Result<(), Box<dyn std::error::Error>> {
    let values = |n: usize| -> Vec<(&'static str, String)> { (0..n).map(|i| ("x-v", i.to_string())).collect() };
    assert!(framed("/", &values(190)).is_ok());
    assert_eq!(
        framed("/", &values(191)).err().as_deref(),
        Some("request headers too large")
    );
    Ok(())
}

#[test]
fn a_c1_control_in_a_header_value_is_refused_and_a_tab_is_not() -> Result<(), Box<dyn std::error::Error>> {
    assert!(framed("/", &[("x-a", "a\u{85}b".to_string())]).is_err());
    assert!(framed("/", &[("x-a", "a\tb".to_string())]).is_ok());
    Ok(())
}

#[tokio::test]
async fn the_spelled_target_names_the_path_and_the_query_when_the_forms_are_known()
-> Result<(), Box<dyn std::error::Error>> {
    let transport = Scripted::new(vec![Ok(r#"{"verdict":"allow"}"#), Ok(r#"{"verdict":"allow"}"#)]);
    let host = Host::new("api.github.com")?;
    let method = HttpMethod::new("GET")?;
    for spelled in ["/a/b?x=1", "/a/b"] {
        let forms = crate::request_filter::TargetForms {
            spelled: spelled.to_string(),
            normalized: spelled.to_string(),
            ambiguous: false,
        };
        let decision = decider(transport.clone())
            .decide(&RequestView {
                host: &host,
                port: 443,
                method: &method,
                path: "/ignored",
                query: None,
                sni: None,
                headers: &[],
                body: None,
                scheme: RequestScheme::Https,
                forms: Some(&forms),
            })
            .await;
        assert_eq!(decision, RequestDecision::Allow);
    }
    let seen = transport.seen()?;
    assert_eq!(seen[0].path, "/a/b");
    assert_eq!(seen[0].query.as_deref(), Some("x=1"));
    assert_eq!(seen[1].path, "/a/b");
    assert_eq!(seen[1].query, None);
    Ok(())
}

#[tokio::test]
async fn an_empty_path_or_port_zero_cannot_be_asked_about() -> Result<(), Box<dyn std::error::Error>> {
    let transport = Scripted::new(vec![]);
    let host = Host::new("api.github.com")?;
    let method = HttpMethod::new("GET")?;
    for (path, port) in [("", 443), ("/x", 0)] {
        let decision = decider(transport.clone())
            .decide(&RequestView {
                host: &host,
                port,
                method: &method,
                path,
                query: None,
                sni: None,
                headers: &[],
                body: None,
                scheme: RequestScheme::Https,
                forms: None,
            })
            .await;
        assert_eq!(status_of(&decision)?.0, 400);
    }
    assert!(transport.seen()?.is_empty());
    Ok(())
}

#[tokio::test]
async fn oversized_or_repeated_removals_and_an_oversized_reply_fail_closed() -> Result<(), Box<dyn std::error::Error>> {
    let many: Vec<String> = (0..191).map(|n| format!("\"x-{n}\"")).collect();
    let too_many = format!(r#"{{"verdict":"allow","remove_headers":[{}]}}"#, many.join(","));
    let repeated = r#"{"verdict":"allow","remove_headers":["x-a","x-a"]}"#.to_string();
    let too_large = " ".repeat(MAX_REPLY_FRAME + 1);
    for reply in [too_many, repeated, too_large] {
        let transport = Scripted::new(vec![Ok(reply.as_str())]);
        let decision = decide(&decider(transport), &headers(), None).await?;
        assert_eq!(status_of(&decision)?.0, 503);
    }
    Ok(())
}
