//! Wire messages exchanged between the HTTP proxy and the detokenization worker.

/// Published to JetStream when the HTTP proxy receives an outbound request.
///
/// The proxy subscribes to `reply_to` on Core NATS and blocks until the worker
/// publishes an [`OutboundHttpResponse`] there.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct OutboundHttpRequest {
    /// HTTP method (e.g. `"POST"`, `"GET"`).
    pub method: String,
    /// Full URL to forward to the AI provider (e.g. `https://api.anthropic.com/v1/messages`).
    pub url: String,
    /// Request headers, including the `Authorization: Bearer tok_...` header.
    ///
    /// Stored as an ordered list of `(name, value)` pairs so that headers with
    /// the same name (e.g. two `Accept` values) are preserved without loss.
    pub headers: Vec<(String, String)>,
    /// Raw request body.
    pub body: Vec<u8>,
    /// Core NATS subject the worker must reply to.
    pub reply_to: String,
    /// Forwarded to the AI provider as `X-Request-Id`.
    pub idempotency_key: String,
}

/// Published by the worker to the `reply_to` Core NATS subject.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct OutboundHttpResponse {
    /// HTTP status code returned by the AI provider.
    pub status: u16,
    /// Response headers from the AI provider.
    ///
    /// Stored as an ordered list of `(name, value)` pairs so that headers with
    /// the same name (e.g. multiple `Set-Cookie` values) are preserved without loss.
    pub headers: Vec<(String, String)>,
    /// Raw response body.
    pub body: Vec<u8>,
    /// Set when an error occurred before or during the upstream call.
    pub error: Option<String>,
}

/// Streaming frame published by the worker to the Core NATS reply subject.
///
/// When the request body contains `"stream": true`, the worker publishes three
/// frame types instead of a single [`OutboundHttpResponse`]:
///
/// 1. `Start` — sent first; carries the HTTP status and response headers.
/// 2. `Chunk` — sent N times; each carries a piece of the response body.
/// 3. `End` — sent last; signals end-of-stream or reports an error that
///    occurred after the `Start` frame was already published.
///
/// Non-streaming responses continue to use [`OutboundHttpResponse`].
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "frame_type", rename_all = "snake_case")]
pub enum StreamFrame {
    Start {
        status: u16,
        headers: Vec<(String, String)>,
    },
    Chunk {
        seq: u64,
        /// Raw body bytes for this chunk, base64-encoded when serialised to JSON.
        data: Vec<u8>,
    },
    End {
        error: Option<String>,
    },
}

#[cfg(test)]
mod stream_frame_tests {
    use super::*;

    #[test]
    fn start_frame_round_trips_json() {
        let frame = StreamFrame::Start {
            status: 200,
            headers: vec![("content-type".to_string(), "text/event-stream".to_string())],
        };
        let json = serde_json::to_string(&frame).unwrap();
        assert!(json.contains("\"frame_type\":\"start\""));
        let decoded: StreamFrame = serde_json::from_str(&json).unwrap();
        assert!(matches!(decoded, StreamFrame::Start { status: 200, .. }));
    }

    #[test]
    fn chunk_frame_round_trips_json() {
        let frame = StreamFrame::Chunk {
            seq: 7,
            data: b"hello, world".to_vec(),
        };
        let json = serde_json::to_string(&frame).unwrap();
        assert!(json.contains("\"frame_type\":\"chunk\""));
        let decoded: StreamFrame = serde_json::from_str(&json).unwrap();
        assert!(
            matches!(decoded, StreamFrame::Chunk { seq: 7, .. }),
            "seq must survive round-trip"
        );
        if let StreamFrame::Chunk { data, .. } = decoded {
            assert_eq!(data, b"hello, world");
        }
    }

    #[test]
    fn end_frame_round_trips_json() {
        let ok = StreamFrame::End { error: None };
        let json = serde_json::to_string(&ok).unwrap();
        assert!(json.contains("\"frame_type\":\"end\""));
        let decoded: StreamFrame = serde_json::from_str(&json).unwrap();
        assert!(matches!(decoded, StreamFrame::End { error: None }));
    }

    #[test]
    fn end_frame_with_error_round_trips_json() {
        let err = StreamFrame::End {
            error: Some("mid-stream error".to_string()),
        };
        let json = serde_json::to_string(&err).unwrap();
        let decoded: StreamFrame = serde_json::from_str(&json).unwrap();
        assert!(matches!(decoded, StreamFrame::End { error: Some(_) }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_round_trips_json() {
        let req = OutboundHttpRequest {
            method: "POST".to_string(),
            url: "https://api.anthropic.com/v1/messages".to_string(),
            headers: vec![(
                "Authorization".to_string(),
                "Bearer tok_anthropic_prod_abc123".to_string(),
            )],
            body: b"{}".to_vec(),
            reply_to: "trogon.proxy.reply.some-uuid".to_string(),
            idempotency_key: "req-id-1".to_string(),
        };

        let json = serde_json::to_string(&req).unwrap();
        let decoded: OutboundHttpRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.method, "POST");
        assert_eq!(decoded.url, "https://api.anthropic.com/v1/messages");
        assert_eq!(decoded.reply_to, "trogon.proxy.reply.some-uuid");
        assert_eq!(decoded.idempotency_key, "req-id-1");
        assert_eq!(decoded.body, b"{}");
    }

    #[test]
    fn request_preserves_duplicate_headers() {
        let req = OutboundHttpRequest {
            method: "GET".to_string(),
            url: "https://api.anthropic.com/v1/messages".to_string(),
            headers: vec![
                ("accept".to_string(), "application/json".to_string()),
                ("accept".to_string(), "text/plain".to_string()),
            ],
            body: vec![],
            reply_to: "trogon.proxy.reply.dup".to_string(),
            idempotency_key: "req-dup".to_string(),
        };

        let json = serde_json::to_string(&req).unwrap();
        let decoded: OutboundHttpRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.headers.len(), 2);
        assert_eq!(
            decoded.headers[0],
            ("accept".to_string(), "application/json".to_string())
        );
        assert_eq!(
            decoded.headers[1],
            ("accept".to_string(), "text/plain".to_string())
        );
    }

    #[test]
    fn response_round_trips_json() {
        let resp = OutboundHttpResponse {
            status: 200,
            headers: vec![("content-type".to_string(), "application/json".to_string())],
            body: b"{\"id\":\"msg_1\"}".to_vec(),
            error: None,
        };

        let json = serde_json::to_string(&resp).unwrap();
        let decoded: OutboundHttpResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.status, 200);
        assert_eq!(decoded.error, None);
        assert_eq!(decoded.body, b"{\"id\":\"msg_1\"}");
    }

    #[test]
    fn response_preserves_multiple_set_cookie_headers() {
        let resp = OutboundHttpResponse {
            status: 200,
            headers: vec![
                ("set-cookie".to_string(), "session=abc; Path=/".to_string()),
                ("set-cookie".to_string(), "prefs=xyz; Path=/".to_string()),
            ],
            body: vec![],
            error: None,
        };

        let json = serde_json::to_string(&resp).unwrap();
        let decoded: OutboundHttpResponse = serde_json::from_str(&json).unwrap();
        let cookies: Vec<_> = decoded
            .headers
            .iter()
            .filter(|(k, _)| k == "set-cookie")
            .collect();
        assert_eq!(
            cookies.len(),
            2,
            "Both Set-Cookie values must survive round-trip"
        );
    }

    #[test]
    fn response_with_error_round_trips() {
        let resp = OutboundHttpResponse {
            status: 500,
            headers: vec![],
            body: vec![],
            error: Some("vault token not found".to_string()),
        };

        let json = serde_json::to_string(&resp).unwrap();
        let decoded: OutboundHttpResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.status, 500);
        assert_eq!(decoded.error.as_deref(), Some("vault token not found"));
    }
}
