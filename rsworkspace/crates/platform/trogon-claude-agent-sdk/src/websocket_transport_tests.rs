//! Replays `tests/fixtures/websocket_transport/goldens.json`, recorded by running the browser SDK's WebSocket
//! transport class against a local server, over a local `tokio-tungstenite` server.
#![cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    allow(
        serde_json_macro,
        reason = "Tests exercise exact upstream JSON fixtures, including malformed and extension shapes."
    )
)]

use std::error::Error as StdError;

use serde_json::{Value, json};
use tokio::{
    io::AsyncWriteExt,
    net::TcpListener,
    sync::{mpsc, oneshot},
    time::timeout,
};
use tokio_tungstenite::{
    accept_async, accept_hdr_async,
    tungstenite::handshake::server::{ErrorResponse, Request as HandshakeRequest, Response as HandshakeResponse},
};

use super::*;

type TestResult = Result<(), Box<dyn StdError>>;
type ServerSocket = WebSocketStream<TcpStream>;

const FIXTURE: &str = include_str!("../tests/fixtures/websocket_transport/goldens.json");

fn golden() -> Result<Value, Box<dyn StdError>> {
    Ok(serde_json::from_str(FIXTURE)?)
}

fn scenario(name: &str) -> Result<Value, Box<dyn StdError>> {
    golden()?["scenarios"]
        .get(name)
        .cloned()
        .ok_or_else(|| Box::<dyn StdError>::from(format!("missing golden {name}")))
}

fn text_of(value: &Value) -> Result<&str, Box<dyn StdError>> {
    value
        .as_str()
        .ok_or_else(|| Box::<dyn StdError>::from(format!("not a string: {value}")))
}

async fn within<T>(future: impl Future<Output = T>) -> Result<T, Box<dyn StdError>> {
    Ok(timeout(Duration::from_secs(5), future).await?)
}

/// Accepts one connection, completes the handshake and runs `script` on the server end.
async fn serve<F, Fut>(script: F) -> Result<String, Box<dyn StdError>>
where
    F: FnOnce(ServerSocket) -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let url = format!("ws://{}/", listener.local_addr()?);
    tokio::spawn(async move {
        if let Ok((stream, _)) = listener.accept().await
            && let Ok(socket) = accept_async(stream).await
        {
            script(socket).await;
        }
    });
    Ok(url)
}

async fn connected(options: WebSocketOptions) -> Result<WebSocketTransport, Box<dyn StdError>> {
    let mut transport = WebSocketTransport::new(options);
    transport.connect().await?;
    Ok(transport)
}

fn transport_error(error: Error) -> Result<WebSocketError, Box<dyn StdError>> {
    match error {
        Error::Transport(inner) => inner
            .downcast::<WebSocketError>()
            .map(|error| *error)
            .map_err(|other| Box::<dyn StdError>::from(format!("not a WebSocketError: {other}"))),
        other => Err(format!("not a transport error: {other:?}").into()),
    }
}

fn close_frame(code: CloseCode, reason: &str) -> Message {
    Message::Close(Some(CloseFrame {
        code,
        reason: reason.into(),
    }))
}

#[tokio::test]
async fn the_requested_delays_are_the_recorded_ones() -> TestResult {
    assert_eq!(
        golden()?["requested_delays"],
        json!(["interval:50000", "timeout:15000"])
    );
    assert_eq!(WEBSOCKET_CONNECT_TIMEOUT.as_millis(), 15_000);
    assert_eq!(WEBSOCKET_KEEP_ALIVE_INTERVAL.as_millis(), 50_000);
    assert_eq!(
        WebSocketError::ConnectTimeout.to_string(),
        "WebSocket connection timeout after 15000ms"
    );
    Ok(())
}

#[tokio::test]
async fn a_session_matches_the_recorded_browser_transport() -> TestResult {
    let golden = scenario("session")?;
    let (received_tx, mut received_rx) = mpsc::unbounded_channel::<String>();
    let url = serve(move |mut socket| async move {
        let frames = [
            Message::text("{\"a\":1}\n{\"b\""),
            Message::text(":2}\n\nnot json\n{\"ccr_event\":{\"x\":1},\"c\":3}\n[1,2]\n\"str\"\n\n"),
            Message::binary(vec![0x7b, 0x7d, 0x0a]),
            Message::text("{\"d\":4}"),
            Message::text("\n"),
        ];
        for frame in frames {
            let _ = socket.send(frame).await;
        }
        let mut count = 0;
        while let Some(Ok(frame)) = socket.next().await {
            if let Message::Text(text) = frame {
                let _ = received_tx.send(text.to_string());
                count += 1;
                if count == 3 {
                    let _ = socket.send(close_frame(CloseCode::Normal, "done")).await;
                }
            }
        }
    })
    .await?;

    let mut options = WebSocketOptions::new(url);
    options.auth_message = Some(AuthMessage::oauth("token-1"));
    let mut transport = connected(options).await?;
    assert_eq!(transport.is_ready(), golden["readyAfterOpen"]);

    let expected: Vec<Value> = golden["messages"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|message| message.is_object())
        .cloned()
        .collect();
    let mut messages = Vec::new();
    while messages.len() < expected.len() {
        let message = within(transport.read_message()).await??;
        messages.push(Value::Object(message.ok_or("the stream ended early")?));
    }
    assert_eq!(messages, expected);

    transport.write("{\"type\":\"user\",\"message\":\"hi\"}\n").await?;
    transport.write("no newline").await?;
    assert!(within(transport.read_message()).await??.is_none());
    assert_eq!(transport.is_ready(), golden["readyAfterClose"]);

    let mut received = Vec::new();
    while let Ok(line) = received_rx.try_recv() {
        received.push(line);
    }
    let recorded = golden["received"].as_array().ok_or("received is not a list")?;
    assert_eq!(recorded.len(), 4);
    assert_eq!(
        received.as_slice(),
        recorded.iter().take(3).map(text_of).collect::<Result<Vec<_>, _>>()?
    );
    assert_eq!(text_of(&recorded[3])?, WEBSOCKET_KEEP_ALIVE_LINE);
    Ok(())
}

#[tokio::test]
async fn the_peer_closing_matches_the_recorded_browser_transport() -> TestResult {
    for name in [
        "abnormal_close",
        "abnormal_close_empty_reason",
        "going_away",
        "close_without_status",
    ] {
        let golden = scenario(name)?;
        let reason = text_of(&golden["reason"])?.to_owned();
        let frame = match golden["code"].as_u64() {
            Some(code) => close_frame(CloseCode::from(u16::try_from(code)?), &reason),
            None => Message::Close(None),
        };
        let url = serve(move |mut socket| async move {
            let _ = socket.send(frame).await;
            while socket.next().await.is_some() {}
        })
        .await?;
        let mut transport = connected(WebSocketOptions::new(url)).await?;
        let outcome = within(transport.read_message()).await?;
        match golden["error"].as_str() {
            Some(expected) => assert_eq!(
                outcome.err().map(|error| error.to_string()).as_deref(),
                Some(expected),
                "{name}"
            ),
            None => assert!(matches!(outcome, Ok(None)), "{name}"),
        }
        assert!(!transport.is_ready());
        assert_eq!(
            transport_error(transport.write("x").await.err().ok_or("write must fail")?)?,
            WebSocketError::NotReady
        );
        assert!(
            matches!(transport.read_message().await, Ok(None)),
            "the error is reported once"
        );
    }
    Ok(())
}

#[tokio::test]
async fn a_dropped_connection_matches_the_recorded_browser_transport() -> TestResult {
    let golden = scenario("abrupt_drop")?;
    let url = serve(|mut socket| async move {
        let _ = socket.send(Message::text("{\"before\":1}\n")).await;
    })
    .await?;
    let mut transport = connected(WebSocketOptions::new(url)).await?;
    let first = within(transport.read_message()).await??.ok_or("no message")?;
    assert_eq!(Value::Object(first), golden["messages"][0]);
    let error = within(transport.read_message())
        .await?
        .err()
        .ok_or("the drop must fail")?;
    assert_eq!(error.to_string(), text_of(&golden["error"])?);
    Ok(())
}

#[tokio::test]
async fn a_protocol_violation_is_a_connection_error() -> TestResult {
    let url = serve(|mut socket| async move {
        let _ = socket.get_mut().write_all(&[0xff, 0x00]).await;
        let _ = socket.next().await;
    })
    .await?;
    let mut transport = connected(WebSocketOptions::new(url)).await?;
    let error = within(transport.read_message())
        .await?
        .err()
        .ok_or("the violation must fail")?;
    assert_eq!(transport_error(error)?, WebSocketError::Connection);
    Ok(())
}

#[tokio::test]
async fn closing_sends_a_normal_closure_and_ends_the_stream() -> TestResult {
    let golden = scenario("client_close")?;
    let (close_tx, close_rx) = oneshot::channel();
    let url = serve(move |mut socket| async move {
        while let Some(Ok(frame)) = socket.next().await {
            if let Message::Close(close) = frame {
                let _ = close_tx.send(close);
                break;
            }
        }
    })
    .await?;
    let mut transport = connected(WebSocketOptions::new(url)).await?;
    transport.close().await?;
    transport.close().await?;
    let close = within(close_rx).await??.ok_or("the close frame carried no status")?;
    assert_eq!(u64::from(u16::from(close.code)), golden["received"][0]["code"]);
    assert_eq!(json!(close.reason.as_str()), golden["received"][0]["reason"]);
    assert_eq!(transport.is_ready(), golden["readyAfterClose"]);
    let error = transport
        .write("{\"type\":\"user\"}\n")
        .await
        .err()
        .ok_or("write must fail")?;
    assert_eq!(error.to_string(), text_of(&golden["writeError"])?);
    assert!(matches!(transport.read_message().await, Ok(None)));
    Ok(())
}

#[tokio::test]
async fn aborting_matches_the_recorded_browser_transport() -> TestResult {
    let golden = scenario("abort")?;
    let (close_tx, close_rx) = oneshot::channel();
    let url = serve(move |mut socket| async move {
        while let Some(Ok(frame)) = socket.next().await {
            if let Message::Close(close) = frame {
                let _ = close_tx.send(close);
                break;
            }
        }
    })
    .await?;
    let token = CancellationToken::new();
    let mut transport = WebSocketTransport::new(WebSocketOptions::new(url)).with_abort(token.clone());
    transport.connect().await?;
    token.cancel();
    let error = within(transport.read_message())
        .await?
        .err()
        .ok_or("the abort must fail")?;
    assert_eq!(error.to_string(), text_of(&golden["error"])?);
    assert_eq!(transport_error(error)?, WebSocketError::Aborted);
    let close = within(close_rx).await??.ok_or("the close frame carried no status")?;
    assert_eq!(u64::from(u16::from(close.code)), golden["received"][0]["code"]);
    assert_eq!(json!(close.reason.as_str()), golden["received"][0]["reason"]);
    let write = transport
        .write("{\"type\":\"user\"}\n")
        .await
        .err()
        .ok_or("write must fail")?;
    assert!(matches!(write, Error::Aborted(_)), "{write:?}");
    assert_eq!(write.to_string(), text_of(&golden["writeError"])?);
    assert!(matches!(transport.read_message().await, Ok(None)));
    Ok(())
}

#[tokio::test]
async fn aborting_before_connecting_matches_the_recorded_browser_transport() -> TestResult {
    let golden = scenario("abort_before_connect")?;
    let token = CancellationToken::new();
    token.cancel();
    let mut transport = WebSocketTransport::new(WebSocketOptions::new("ws://127.0.0.1:1/")).with_abort(token);
    let error = transport.connect().await.err().ok_or("connect must fail")?;
    assert_eq!(error.to_string(), text_of(&golden["readError"])?);
    let write = transport
        .write("{\"type\":\"user\"}\n")
        .await
        .err()
        .ok_or("write must fail")?;
    assert_eq!(write.to_string(), text_of(&golden["writeError"])?);
    Ok(())
}

#[tokio::test]
async fn an_unusable_url_is_refused_with_the_recorded_text() -> TestResult {
    for name in ["scheme_http", "scheme_https", "not_a_url"] {
        let golden = scenario(name)?;
        let mut transport = WebSocketTransport::new(WebSocketOptions::new(text_of(&golden["url"])?));
        let error = transport.connect().await.err().ok_or("connect must fail")?;
        assert_eq!(error.to_string(), text_of(&golden["error"])?, "{name}");
        assert!(!transport.is_ready());
    }
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_connection_that_never_opens_times_out_after_fifteen_seconds() -> TestResult {
    let golden = scenario("connect_timeout")?;
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let url = format!("ws://{}/", listener.local_addr()?);
    tokio::spawn(async move {
        let _held = listener.accept().await;
        pending::<()>().await;
    });
    let started = Instant::now();
    let mut transport = WebSocketTransport::new(WebSocketOptions::new(url));
    let error = transport.connect().await.err().ok_or("connect must time out")?;
    assert_eq!(error.to_string(), text_of(&golden["error"])?);
    assert_eq!(transport_error(error)?, WebSocketError::ConnectTimeout);
    assert!(started.elapsed() >= Duration::from_millis(15_000));
    assert!(!transport.is_ready());
    Ok(())
}

#[tokio::test]
async fn a_refused_connection_matches_the_recorded_browser_transport() -> TestResult {
    let golden = scenario("connect_refused")?;
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let url = format!("ws://{}/", listener.local_addr()?);
    drop(listener);
    let mut transport = WebSocketTransport::new(WebSocketOptions::new(url));
    let error = transport.connect().await.err().ok_or("connect must fail")?;
    assert_eq!(error.to_string(), text_of(&golden["error"])?);
    assert_eq!(transport_error(error)?, WebSocketError::Connection);
    Ok(())
}

#[tokio::test]
async fn the_keep_alive_is_sent_on_its_interval_while_reading() -> TestResult {
    // The 50 second interval itself is pinned above; a short one drives the same timer on the real clock, since a
    // paused clock jumps ahead while the server end is still waiting on the socket.
    let interval = Duration::from_millis(200);
    let (seen_tx, mut seen_rx) = mpsc::unbounded_channel::<(Instant, String)>();
    let url = serve(move |mut socket| async move {
        while let Some(Ok(frame)) = socket.next().await {
            if let Message::Text(text) = frame {
                let _ = seen_tx.send((Instant::now(), text.to_string()));
            }
        }
    })
    .await?;
    let mut transport = WebSocketTransport::new(WebSocketOptions::new(url));
    transport.keep_alive_interval = interval;
    let started = Instant::now();
    transport.connect().await?;
    let reading = tokio::spawn(async move { transport.read_message().await });
    let (first_at, first) = within(seen_rx.recv()).await?.ok_or("no first keep-alive")?;
    let (_, second) = within(seen_rx.recv()).await?.ok_or("no second keep-alive")?;
    reading.abort();
    assert_eq!(first, WEBSOCKET_KEEP_ALIVE_LINE);
    assert_eq!(second, WEBSOCKET_KEEP_ALIVE_LINE);
    assert!(first_at - started >= interval, "{:?}", first_at - started);
    Ok(())
}

#[tokio::test]
async fn a_read_dropped_midway_loses_neither_a_message_nor_a_partial_line() -> TestResult {
    let (release_tx, release_rx) = oneshot::channel::<()>();
    let url = serve(move |mut socket| async move {
        let _ = socket.send(Message::text("{\"a\":1}\n{\"b\"")).await;
        let _ = release_rx.await;
        let _ = socket.send(Message::text(":2}\n")).await;
        pending::<()>().await;
    })
    .await?;
    let mut transport = connected(WebSocketOptions::new(url)).await?;
    let first = within(transport.read_message()).await??.ok_or("no first message")?;
    assert_eq!(Value::Object(first), json!({"a": 1}));
    assert!(
        timeout(Duration::from_millis(50), transport.read_message())
            .await
            .is_err()
    );
    let _ = release_tx.send(());
    let second = within(transport.read_message()).await??.ok_or("no second message")?;
    assert_eq!(Value::Object(second), json!({"b": 2}));
    Ok(())
}

// The handshake callback's signature is tungstenite's, large error response included.
#[allow(clippy::result_large_err)]
async fn handshake_header(
    options: impl FnOnce(String) -> WebSocketOptions,
) -> Result<Option<String>, Box<dyn StdError>> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let url = format!("ws://{}/", listener.local_addr()?);
    let (seen_tx, seen_rx) = oneshot::channel();
    tokio::spawn(async move {
        if let Ok((stream, _)) = listener.accept().await {
            let callback = move |request: &HandshakeRequest,
                                 response: HandshakeResponse|
                  -> Result<HandshakeResponse, ErrorResponse> {
                let seen = request
                    .headers()
                    .get("x-remote-token")
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_owned);
                let _ = seen_tx.send(seen);
                Ok(response)
            };
            if let Ok(_socket) = accept_hdr_async(stream, callback).await {
                pending::<()>().await;
            }
        }
    });
    let _transport = connected(options(url)).await?;
    Ok(within(seen_rx).await??)
}

fn with_header(url: String, delivery: HeaderDelivery) -> WebSocketOptions {
    let mut options = WebSocketOptions::new(url);
    options
        .headers
        .insert("x-remote-token".to_owned(), "secret-value".to_owned());
    options.header_delivery = delivery;
    options
}

#[tokio::test]
async fn headers_are_ignored_by_default_like_the_browser_transport() -> TestResult {
    assert_eq!(
        handshake_header(|url| with_header(url, HeaderDelivery::default())).await?,
        None
    );
    Ok(())
}

#[tokio::test]
async fn headers_reach_the_handshake_when_asked() -> TestResult {
    let seen = handshake_header(|url| with_header(url, HeaderDelivery::Handshake)).await?;
    assert_eq!(seen.as_deref(), Some("secret-value"));
    Ok(())
}

#[tokio::test]
async fn an_unsendable_header_is_refused_before_connecting() -> TestResult {
    for (name, value) in [("bad name", "value"), ("x-remote-token", "line\nbreak")] {
        let mut options = WebSocketOptions::new("ws://127.0.0.1:1/");
        options.header_delivery = HeaderDelivery::Handshake;
        options.headers.insert(name.to_owned(), value.to_owned());
        let error = WebSocketTransport::new(options)
            .connect()
            .await
            .err()
            .ok_or("connect must fail")?;
        assert_eq!(error.to_string(), format!("invalid WebSocket handshake header: {name}"));
        assert_eq!(
            transport_error(error)?,
            WebSocketError::InvalidHeader { name: name.to_owned() }
        );
    }
    Ok(())
}

#[tokio::test]
async fn a_transport_that_never_connected_is_not_ready() -> TestResult {
    let mut transport = WebSocketTransport::new(WebSocketOptions::new("ws://127.0.0.1:1/"));
    assert!(!transport.is_ready());
    let error = transport.write("x").await.err().ok_or("write must fail")?;
    assert_eq!(error.to_string(), "WebSocketTransport is not ready for writing");
    transport.end_input().await?;
    assert!(matches!(transport.read_message().await, Ok(None)));
    Ok(())
}

#[test]
fn the_auth_line_is_the_recorded_one_and_the_debug_output_hides_credentials() -> TestResult {
    let golden = scenario("session")?;
    let message = AuthMessage::oauth("token-1");
    assert_eq!(message.line(), text_of(&golden["received"][0])?);
    assert_eq!(message.credential.token.expose(), "token-1");
    assert!(!format!("{message:?}").contains("token-1"));

    let mut options = WebSocketOptions::new("wss://example.invalid/path?key=url-secret");
    options
        .headers
        .insert("authorization".to_owned(), "header-secret".to_owned());
    options.auth_message = Some(message);
    let debug = format!("{options:?}");
    for secret in ["token-1", "url-secret", "header-secret"] {
        assert!(!debug.contains(secret), "{debug}");
    }
    assert!(debug.contains("authorization"));
    assert_eq!(options.header_delivery, HeaderDelivery::Ignored);
    Ok(())
}
