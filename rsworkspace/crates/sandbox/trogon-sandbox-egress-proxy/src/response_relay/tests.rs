use super::*;
use crate::secret_substitution::tests::{API_REAL, fixture, scope};
use std::sync::{Arc, Mutex};

type TestResult<T> = Result<T, Box<dyn std::error::Error>>;

const RELAYED: HopByHop = HopByHop::Relayed;

fn stripped(version: RequestVersion) -> HopByHop {
    HopByHop::Stripped {
        client: AnsweredRequest::of_version("GET", version, false),
    }
}

async fn relay(
    response: &[u8],
    scrub: Option<&ResponseScrub<'_>>,
    hook: Option<&ResponseHook>,
    hops: HopByHop,
) -> (Result<Relayed, HttpInterceptError>, String) {
    let mut upstream: &[u8] = response;
    let mut client = Vec::new();
    let result = relay_scrubbed(&mut upstream, &mut client, "GET", scrub, hook, false, hops).await;
    (result, String::from_utf8_lossy(&client).into_owned())
}

fn chunked_head() -> &'static str {
    "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n"
}

fn headers(list: &[(&str, &str)]) -> Vec<(String, String)> {
    list.iter()
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect()
}

fn status(head: &str) -> TestResult<ResponseStatus> {
    Ok(ResponseStatus::parse(head).map_err(|error| format!("{error:?}"))?)
}

#[test]
fn folded_lines_follow_the_field_they_continue_when_hop_by_hop_fields_are_stripped() -> TestResult<()> {
    let head = "HTTP/1.0 200 OK\r\nConnection: x-drop\r\nX-Drop: a\r\n b\r\nX-Keep: 1\r\n c\r\n\r\n";
    assert_eq!(strip_hop_by_hop(head), "HTTP/1.1 200 OK\r\nX-Keep: 1\r\n c\r\n\r\n");
    Ok(())
}

#[tokio::test]
async fn a_chunk_size_line_that_never_ends_is_refused() -> TestResult<()> {
    let response = format!(
        "{}5\r\nhello\r\n{}",
        chunked_head(),
        "a".repeat(MAX_CHUNK_LINE_BYTES + 10)
    );
    let (result, _) = relay(response.as_bytes(), None, None, RELAYED).await;
    assert!(matches!(result, Err(HttpInterceptError::MalformedRequest(_))));
    Ok(())
}

#[tokio::test]
async fn an_upstream_that_closes_inside_the_chunk_framing_is_refused() -> TestResult<()> {
    let response = format!("{}5\r\nhello\r\n3", chunked_head());
    let (result, _) = relay(response.as_bytes(), None, None, RELAYED).await;
    assert!(matches!(result, Err(HttpInterceptError::MalformedRequest(_))));
    Ok(())
}

#[tokio::test]
async fn trailers_past_the_limit_are_refused() -> TestResult<()> {
    let trailer = format!("X-Pad: {}\r\n", "p".repeat(500));
    let response = format!("{}0\r\n{}\r\n", chunked_head(), trailer.repeat(20));
    let (result, _) = relay(response.as_bytes(), None, None, RELAYED).await;
    assert!(matches!(result, Err(HttpInterceptError::MalformedRequest(_))));
    Ok(())
}

#[tokio::test]
async fn chunk_data_not_followed_by_its_line_end_is_refused_in_every_framing() -> TestResult<()> {
    let response = format!("{}5\r\nhelloXX\r\n0\r\n\r\n", chunked_head());
    for hops in [
        RELAYED,
        stripped(RequestVersion::Http10),
        stripped(RequestVersion::Http11),
    ] {
        let (result, _) = relay(response.as_bytes(), None, None, hops).await;
        assert!(matches!(result, Err(HttpInterceptError::MalformedRequest(_))));
    }
    Ok(())
}

#[tokio::test]
async fn a_chunked_response_keeps_or_drops_its_length_field_as_relayed() -> TestResult<()> {
    let plain = format!("{}3\r\nabc\r\n0\r\n\r\n", chunked_head());
    let (result, out) = relay(plain.as_bytes(), None, None, RELAYED).await;
    result.map_err(|error| format!("{error:?}"))?;
    assert_eq!(out, plain);

    let with_length = "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nContent-Length: 3\r\n\r\n3\r\nabc\r\n0\r\n\r\n";
    let (result, out) = relay(with_length.as_bytes(), None, None, RELAYED).await;
    result.map_err(|error| format!("{error:?}"))?;
    assert!(!out.contains("Content-Length"));
    assert!(out.contains("Transfer-Encoding: chunked"));
    Ok(())
}

#[tokio::test]
async fn a_stripped_response_reaches_its_hook_without_hop_by_hop_fields() -> TestResult<()> {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&seen);
    let hook = ResponseHook::new(move |status, headers| {
        if let Ok(mut seen) = sink.lock() {
            seen.push((status, headers.to_vec()));
        }
    });
    let response = "HTTP/1.1 200 OK\r\nConnection: close\r\nX-One: 1\r\nContent-Length: 2\r\n\r\nok";
    let (result, _) = relay(response.as_bytes(), None, Some(&hook), stripped(RequestVersion::Http11)).await;
    result.map_err(|error| format!("{error:?}"))?;
    let seen = seen.lock().map_err(|_| "poisoned")?;
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].0, 200);
    assert!(seen[0].1.iter().any(|(name, _)| name == "X-One"));
    assert!(
        !seen[0]
            .1
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("connection"))
    );
    Ok(())
}

#[tokio::test]
async fn a_body_too_big_to_buffer_is_streamed_to_an_http_1_0_client_without_a_length() -> TestResult<()> {
    let fx = fixture()?;
    let scope = scope(&fx, "api.example.com")?;
    let scrub = ResponseScrub::new(Some(&scope), None, BodyLimit::new(4)).ok_or("expected a scrub")?;
    let body = format!("x{API_REAL}");
    let response = format!("HTTP/1.0 200 OK\r\nContent-Length: {}\r\n\r\n{body}", body.len());
    for hops in [RELAYED, stripped(RequestVersion::Http10)] {
        let (result, out) = relay(response.as_bytes(), Some(&scrub), None, hops).await;
        let relayed = result.map_err(|error| format!("{error:?}"))?;
        assert_eq!(relayed.connection, Connection::Close);
        assert!(!out.to_ascii_lowercase().contains("content-length"));
        assert!(out.ends_with(&format!("x{}", fx.api)));
        assert!(!out.contains(API_REAL));
    }
    Ok(())
}

#[tokio::test]
async fn a_relayed_length_is_rewritten_once_when_scrubbing_changes_the_body() -> TestResult<()> {
    let fx = fixture()?;
    assert_ne!(fx.api.len(), API_REAL.len());
    let scope = scope(&fx, "api.example.com")?;
    let scrub = ResponseScrub::new(Some(&scope), None, BodyLimit::DEFAULT).ok_or("expected a scrub")?;
    let length = API_REAL.len();
    let response = format!("HTTP/1.1 200 OK\r\nContent-Length: {length}\r\nContent-Length: {length}\r\n\r\n{API_REAL}");
    let (result, out) = relay(response.as_bytes(), Some(&scrub), None, RELAYED).await;
    result.map_err(|error| format!("{error:?}"))?;
    assert_eq!(out.matches("Content-Length").count(), 1);
    assert!(out.contains(&format!("Content-Length: {}\r\n", fx.api.len())));
    assert!(out.ends_with(&fx.api));
    Ok(())
}

#[tokio::test]
async fn a_relayed_http_1_1_body_too_big_to_buffer_is_chunked() -> TestResult<()> {
    let fx = fixture()?;
    let scope = scope(&fx, "api.example.com")?;
    let scrub = ResponseScrub::new(Some(&scope), None, BodyLimit::new(4)).ok_or("expected a scrub")?;
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{API_REAL}",
        API_REAL.len()
    );
    let (result, out) = relay(response.as_bytes(), Some(&scrub), None, RELAYED).await;
    result.map_err(|error| format!("{error:?}"))?;
    assert!(out.contains("Transfer-Encoding: chunked\r\n"));
    assert!(!out.contains("Content-Length"));
    Ok(())
}

#[tokio::test]
async fn a_body_that_only_outgrows_the_limit_once_the_held_back_tail_is_released_is_refused() -> TestResult<()> {
    let fx = fixture()?;
    let scope = scope(&fx, "api.example.com")?;
    let bodies = [
        format!("{}{API_REAL}", "x".repeat(8)),
        format!("{API_REAL}{}", "x".repeat(512)),
    ];
    let mut refused = 0;
    for body in bodies {
        let response = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}", body.len());
        let grown = body.len() - API_REAL.len() + fx.api.len();
        for limit in (body.len()..grown).rev() {
            let scrub = ResponseScrub::new(Some(&scope), None, BodyLimit::new(limit)).ok_or("expected a scrub")?;
            let (result, out) = relay(
                response.as_bytes(),
                Some(&scrub),
                None,
                stripped(RequestVersion::Http11),
            )
            .await;
            result.map_err(|error| format!("{error:?}"))?;
            if out.starts_with("HTTP/1.1 502") {
                refused += 1;
            }
        }
    }
    assert!(refused > 0);
    Ok(())
}

#[test]
fn a_status_line_needs_an_http_1_version_and_a_code() -> TestResult<()> {
    assert!(ResponseStatus::parse("HTTP/2 200 OK\r\n\r\n").is_err());
    assert!(ResponseStatus::parse("HTTP/1.1 99 Low\r\n\r\n").is_err());
    assert!(!status("HTTP/1.0 200 OK\r\n\r\n")?.http11);
    Ok(())
}

#[test]
fn response_framing_follows_the_status_codings_and_lengths() -> TestResult<()> {
    let ok = status("HTTP/1.1 200 OK\r\n\r\n")?;
    let switching = status("HTTP/1.1 101 Switching Protocols\r\n\r\n")?;
    assert!(matches!(
        response_framing("GET", switching, &[]).map_err(|e| format!("{e:?}"))?,
        Framing::UntilClose
    ));
    assert!(matches!(
        response_framing("GET", ok, &headers(&[("Transfer-Encoding", "gzip")])).map_err(|e| format!("{e:?}"))?,
        Framing::UntilClose
    ));
    assert!(matches!(
        response_framing("GET", ok, &headers(&[("Content-Length", "x")])),
        Err(HttpInterceptError::MalformedRequest(_))
    ));
    assert!(matches!(
        response_framing("GET", ok, &headers(&[("Content-Length", "1, 2")])),
        Err(HttpInterceptError::MalformedRequest(_))
    ));
    assert!(matches!(
        response_framing("GET", ok, &headers(&[("Content-Length", "99999999999999999999999")])),
        Err(HttpInterceptError::MalformedRequest(_))
    ));
    Ok(())
}

#[test]
fn an_identity_coding_is_not_compression() -> TestResult<()> {
    assert!(!is_compressed(&headers(&[("Transfer-Encoding", "identity, chunked")])));
    assert!(is_compressed(&headers(&[("Transfer-Encoding", "gzip, chunked")])));
    Ok(())
}

#[tokio::test]
async fn a_watched_writer_notes_writes_and_passes_a_shutdown_through() -> TestResult<()> {
    let mut sink = Vec::new();
    let mut watched = WriteWatch::new(&mut sink);
    assert!(!watched.wrote());
    watched.write_all(b"x").await?;
    watched.flush().await?;
    watched.shutdown().await?;
    assert!(watched.wrote());
    Ok(())
}
