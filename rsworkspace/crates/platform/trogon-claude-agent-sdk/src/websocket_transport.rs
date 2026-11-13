//! The TypeScript browser SDK's WebSocket transport as an implementation of [`Transport`].

use std::{collections::VecDeque, future::pending, time::Duration};

use futures_util::{FutureExt, SinkExt, StreamExt};
use indexmap::IndexMap;
use serde::Serialize;
use tokio::{
    net::TcpStream,
    time::{Instant, Interval, MissedTickBehavior, interval_at, timeout},
};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async,
    tungstenite::{
        Error as SocketError, Message,
        client::IntoClientRequest,
        error::ProtocolError,
        http::{HeaderName, HeaderValue, Request},
        protocol::{CloseFrame, frame::coding::CloseCode},
    },
};
use tokio_util::sync::CancellationToken;

use crate::{AbortError, Error, JsonObject, SensitiveString, Transport, WebSocketError};

/// TypeScript `oA`: how long the connection may take to open.
const CONNECT_TIMEOUT: Duration = Duration::from_millis(15_000);
/// TypeScript `g4`: the delay between keep-alive frames.
const KEEP_ALIVE_INTERVAL: Duration = Duration::from_millis(50_000);
const KEEP_ALIVE_LINE: &str = "{\"type\":\"keep_alive\"}\n";
const CLOSE_REASON: &str = "Normal closure";
/// The code a browser reports for a close frame without a status.
const NO_STATUS_CODE: u16 = 1005;
/// The code a browser reports when the connection drops without a close frame.
const ABNORMAL_CODE: u16 = 1006;

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// TypeScript `OAuthCredential`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OAuthCredential {
    pub token: SensitiveString,
}

/// TypeScript `AuthMessage`: the line sent first when the socket opens, `{"type":"auth","credential":{...}}`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthMessage {
    pub credential: OAuthCredential,
}

impl AuthMessage {
    pub fn oauth(token: impl Into<SensitiveString>) -> Self {
        Self {
            credential: OAuthCredential { token: token.into() },
        }
    }

    fn line(&self) -> String {
        #[derive(Serialize)]
        struct Credential<'a> {
            #[serde(rename = "type")]
            kind: &'static str,
            token: &'a str,
        }
        #[derive(Serialize)]
        struct Line<'a> {
            #[serde(rename = "type")]
            kind: &'static str,
            credential: Credential<'a>,
        }
        let line = Line {
            kind: "auth",
            credential: Credential {
                kind: "oauth",
                token: self.credential.token.expose(),
            },
        };
        let mut text = serde_json::to_string(&line).unwrap_or_default();
        text.push('\n');
        text
    }
}

/// Whether [`WebSocketOptions::headers`] reach the server.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum HeaderDelivery {
    /// TypeScript: the option is accepted and never used, because a browser WebSocket cannot set headers.
    #[default]
    Ignored,
    /// Opt-in: send the headers on the upgrade request, which a native client can do and a browser cannot.
    Handshake,
}

/// TypeScript `WebSocketOptions`.
#[derive(Clone, Eq, PartialEq)]
pub struct WebSocketOptions {
    /// A `ws://` or `wss://` URL.
    pub url: String,
    /// TypeScript `headers`. Ignored unless [`WebSocketOptions::header_delivery`] is [`HeaderDelivery::Handshake`].
    pub headers: IndexMap<String, String>,
    pub header_delivery: HeaderDelivery,
    pub auth_message: Option<AuthMessage>,
}

impl WebSocketOptions {
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            headers: IndexMap::new(),
            header_delivery: HeaderDelivery::default(),
            auth_message: None,
        }
    }
}

impl std::fmt::Debug for WebSocketOptions {
    /// Neither the URL nor the header values appear: both commonly carry credentials.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebSocketOptions")
            .field("headers", &self.headers.keys().collect::<Vec<_>>())
            .field("header_delivery", &self.header_delivery)
            .field("auth_message", &self.auth_message.is_some())
            .finish_non_exhaustive()
    }
}

struct Connection {
    socket: Socket,
    keep_alive: Interval,
}

enum Event {
    Aborted,
    KeepAlive,
    Frame(Option<Result<Message, SocketError>>),
}

/// A WebSocket connection to a remote Claude Code, speaking the newline-delimited JSON the CLI speaks on stdio.
///
/// Inbound text frames carry JSON lines, which may be split across frames or share one; binary frames are ignored,
/// lines that are not JSON are dropped, and a `ccr_event` key is removed from every object. JSON values that are
/// not objects are dropped, because [`Transport::read_message`] yields objects. Outbound documents are sent
/// verbatim. The keep-alive frame is sent while the driver is waiting in [`Transport::read_message`].
pub struct WebSocketTransport {
    options: WebSocketOptions,
    abort: Option<CancellationToken>,
    connection: Option<Connection>,
    ready: bool,
    peer_closed: bool,
    pending: VecDeque<JsonObject>,
    partial_line: String,
    exit_error: Option<WebSocketError>,
    ended: bool,
}

impl WebSocketTransport {
    pub fn new(options: WebSocketOptions) -> Self {
        Self {
            options,
            abort: None,
            connection: None,
            ready: false,
            peer_closed: false,
            pending: VecDeque::new(),
            partial_line: String::new(),
            exit_error: None,
            ended: false,
        }
    }

    /// TypeScript `abortController`: cancelling the token closes the socket and ends the stream with
    /// [`WebSocketError::Aborted`]; a later write fails with `Operation aborted`.
    #[must_use]
    pub fn with_abort(mut self, token: CancellationToken) -> Self {
        self.abort = Some(token);
        self
    }

    fn request(&self) -> Result<Request<()>, WebSocketError> {
        let url = url::Url::parse(&self.options.url).map_err(|_| WebSocketError::InvalidUrl)?;
        if !matches!(url.scheme(), "ws" | "wss") {
            return Err(WebSocketError::UnsupportedScheme);
        }
        let mut request = url.as_str().into_client_request()?;
        if self.options.header_delivery == HeaderDelivery::Handshake {
            for (name, value) in &self.options.headers {
                let (header, value) =
                    header(name, value).ok_or_else(|| WebSocketError::InvalidHeader { name: name.clone() })?;
                request.headers_mut().insert(header, value);
            }
        }
        Ok(request)
    }

    /// TypeScript `close()`: a close frame with code 1000 when the socket is open, without waiting for the peer.
    fn close_socket(&mut self) {
        self.ready = false;
        if let Some(mut connection) = self.connection.take() {
            if !self.peer_closed {
                let close = Message::Close(Some(CloseFrame {
                    code: CloseCode::Normal,
                    reason: CLOSE_REASON.into(),
                }));
                let _ = connection.socket.send(close).now_or_never();
            }
        }
    }

    fn end(&mut self, error: Option<WebSocketError>) {
        self.ready = false;
        self.connection = None;
        self.ended = true;
        if self.exit_error.is_none() {
            self.exit_error = error;
        }
    }

    fn accumulate(&mut self, text: &str) {
        let mut combined = std::mem::take(&mut self.partial_line);
        combined.push_str(text);
        let mut lines: Vec<&str> = combined.split('\n').collect();
        self.partial_line = lines.pop().unwrap_or_default().to_owned();
        for line in lines {
            if let Ok(serde_json::Value::Object(mut object)) = serde_json::from_str(line) {
                object.shift_remove("ccr_event");
                self.pending.push_back(object);
            }
        }
    }

    fn peer_close(&mut self, frame: Option<CloseFrame>) {
        let (code, reason) = frame.map_or((NO_STATUS_CODE, String::new()), |frame| {
            (u16::from(frame.code), frame.reason.to_string())
        });
        self.peer_closed = true;
        self.ready = false;
        if code != 1000 && code != 1001 {
            self.exit_error = Some(WebSocketError::ClosedAbnormally { code, reason });
        }
    }

    fn frame(&mut self, frame: Option<Result<Message, SocketError>>) {
        match frame {
            Some(Ok(Message::Text(text))) => self.accumulate(text.as_str()),
            Some(Ok(Message::Close(frame))) => self.peer_close(frame),
            Some(Ok(_)) => {}
            None | Some(Err(SocketError::Protocol(ProtocolError::ResetWithoutClosingHandshake))) => {
                let dropped = (!self.peer_closed).then(|| WebSocketError::ClosedAbnormally {
                    code: ABNORMAL_CODE,
                    reason: String::new(),
                });
                self.end(dropped);
            }
            Some(Err(_)) => self.end(Some(WebSocketError::Connection)),
        }
    }

    async fn step(&mut self) {
        let Some(connection) = self.connection.as_mut() else {
            self.end(None);
            return;
        };
        let event = tokio::select! {
            biased;
            () = cancelled(self.abort.as_ref()) => Event::Aborted,
            _ = connection.keep_alive.tick(), if !self.peer_closed => Event::KeepAlive,
            frame = connection.socket.next() => Event::Frame(frame),
        };
        match event {
            Event::Aborted => {
                self.close_socket();
                self.exit_error = Some(WebSocketError::Aborted);
                self.end(None);
            }
            Event::KeepAlive => {
                let _ = connection.socket.send(Message::text(KEEP_ALIVE_LINE)).await;
            }
            Event::Frame(frame) => self.frame(frame),
        }
    }
}

fn header(name: &str, value: &str) -> Option<(HeaderName, HeaderValue)> {
    Some((
        HeaderName::from_bytes(name.as_bytes()).ok()?,
        HeaderValue::from_str(value).ok()?,
    ))
}

async fn cancelled(token: Option<&CancellationToken>) {
    match token {
        Some(token) => token.cancelled().await,
        None => pending().await,
    }
}

impl Transport for WebSocketTransport {
    async fn connect(&mut self) -> Result<(), Error> {
        let request = self.request()?;
        let opened = tokio::select! {
            biased;
            () = cancelled(self.abort.as_ref()) => return Err(WebSocketError::Aborted.into()),
            opened = timeout(CONNECT_TIMEOUT, connect_async(request)) => opened,
        };
        let (mut socket, _response) = opened
            .map_err(|_| WebSocketError::ConnectTimeout)?
            .map_err(WebSocketError::from)?;
        self.ready = true;
        if let Some(auth) = &self.options.auth_message {
            let _ = socket.send(Message::text(auth.line())).await;
        }
        let mut keep_alive = interval_at(Instant::now() + KEEP_ALIVE_INTERVAL, KEEP_ALIVE_INTERVAL);
        keep_alive.set_missed_tick_behavior(MissedTickBehavior::Delay);
        self.connection = Some(Connection { socket, keep_alive });
        Ok(())
    }

    async fn write(&mut self, data: &str) -> Result<(), Error> {
        if self.abort.as_ref().is_some_and(CancellationToken::is_cancelled) {
            return Err(AbortError::OPERATION.into());
        }
        let (true, Some(connection)) = (self.ready, self.connection.as_mut()) else {
            return Err(WebSocketError::NotReady.into());
        };
        connection
            .socket
            .send(Message::text(data))
            .await
            .map_err(WebSocketError::from)?;
        Ok(())
    }

    async fn read_message(&mut self) -> Result<Option<JsonObject>, Error> {
        loop {
            if let Some(message) = self.pending.pop_front() {
                return Ok(Some(message));
            }
            if self.ended {
                return self.exit_error.take().map_or(Ok(None), |error| Err(error.into()));
            }
            self.step().await;
        }
    }

    async fn close(&mut self) -> Result<(), Error> {
        self.abort = None;
        self.close_socket();
        self.ended = true;
        Ok(())
    }

    fn is_ready(&self) -> bool {
        self.ready
    }

    async fn end_input(&mut self) -> Result<(), Error> {
        Ok(())
    }
}

#[cfg(test)]
#[path = "websocket_transport_tests.rs"]
mod tests;
