use bytes::Bytes;
use http_body_util::{BodyExt, Full, StreamBody, combinators::BoxBody};
use serde::Serialize;

/// One scripted answer of the fake apiserver.
pub(super) struct FakeReply {
    status: u16,
    date: Option<&'static str>,
    body: Vec<u8>,
    kind: BodyKind,
}

enum BodyKind {
    Complete,
    Broken,
    Hanging,
}

pub(super) type FakeBody = BoxBody<Bytes, std::io::Error>;

const SUCCESS_STATUS: &str = r#"{"kind":"Status","apiVersion":"v1","status":"Success","code":200}"#;

impl FakeReply {
    pub(super) fn object<T: Serialize>(object: &T) -> Result<Self, serde_json::Error> {
        Ok(Self::text(200, serde_json::to_string(object)?))
    }

    /// A list response holding `items`, each an already serialized object.
    pub(super) fn list<T: Serialize>(api_version: &str, kind: &str, items: &[T]) -> Result<Self, serde_json::Error> {
        let items = items
            .iter()
            .map(serde_json::to_string)
            .collect::<Result<Vec<_>, _>>()?
            .join(",");
        Ok(Self::text(
            200,
            format!(
                r#"{{"apiVersion":"{api_version}","kind":"{kind}","metadata":{{"resourceVersion":"1"}},"items":[{items}]}}"#
            ),
        ))
    }

    /// A watch stream carrying `events`, one JSON document per line.
    pub(super) fn watch_events(events: &[&str]) -> Self {
        Self::text(200, format!("{}\n", events.join("\n")))
    }

    pub(super) fn text(status: u16, body: impl Into<String>) -> Self {
        Self {
            status,
            date: None,
            body: body.into().into_bytes(),
            kind: BodyKind::Complete,
        }
    }

    pub(super) fn deleted() -> Self {
        Self::text(200, SUCCESS_STATUS)
    }

    pub(super) fn failure(code: u16, reason: &str) -> Self {
        Self::text(
            code,
            format!(
                r#"{{"kind":"Status","apiVersion":"v1","status":"Failure","message":"scripted {reason}","reason":"{reason}","code":{code}}}"#
            ),
        )
    }

    pub(super) fn with_date(mut self, date: &'static str) -> Self {
        self.date = Some(date);
        self
    }

    /// A response whose body fails midway.
    pub(super) fn broken_body() -> Self {
        Self {
            kind: BodyKind::Broken,
            ..Self::text(200, "")
        }
    }

    /// A response whose body never ends, like a watch with no events.
    pub(super) fn hanging() -> Self {
        Self {
            kind: BodyKind::Hanging,
            ..Self::text(200, "")
        }
    }

    pub(super) fn into_response(self) -> Result<http::Response<FakeBody>, http::Error> {
        let mut builder = http::Response::builder()
            .status(self.status)
            .header(http::header::CONTENT_TYPE, "application/json");
        if let Some(date) = self.date {
            builder = builder.header(http::header::DATE, date);
        }
        let body: FakeBody = match self.kind {
            BodyKind::Broken => {
                let frames = futures::stream::iter(vec![Err(std::io::Error::other("broken body"))]);
                StreamBody::new(frames).boxed()
            }
            BodyKind::Hanging => StreamBody::new(futures::stream::pending::<
                Result<hyper::body::Frame<Bytes>, std::io::Error>,
            >())
            .boxed(),
            BodyKind::Complete => Full::new(Bytes::from(self.body))
                .map_err(|never| match never {})
                .boxed(),
        };
        builder.body(body)
    }
}
