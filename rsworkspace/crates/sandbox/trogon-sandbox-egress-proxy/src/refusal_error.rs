use std::time::SystemTime;

use crate::framed_head::{AnsweredRequest, FramedAnswer, FramedHead, status_text};
use crate::head_parse_error::HeadParseError;
use crate::host_mismatch_error::HostMismatchError;
use crate::request_decision::RequestDenial;
use crate::sigv4_resign::Sigv4RefusalError;
use crate::status_code::StatusCode;

/// A request the proxy answers itself instead of forwarding.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RefusalError {
    #[error("the TRACE method is not forwarded")]
    EchoMethod,
    #[error("malformed request (Parse Error: {0})")]
    Unparsed(HeadParseError),
    /// A parse error the workload's HTTP stack raises after the server has handed the request to
    /// the proxy, which leaves its handler to answer it: in a tunnel the handler
    /// has started, so the socket is destroyed with nothing written.
    #[error("malformed request (Parse Error: {0})")]
    UnparsedBody(HeadParseError),
    /// A request-target a strict HTTP parser refuses (`HPE_INVALID_URL`), which is
    /// answered with its own reason.
    #[error("malformed request-target: percent-encode its bytes outside printable ASCII")]
    UnparsedTarget,
    /// A request a strict HTTP server takes for an upgrade: the strict parser makes one of an
    /// HTTP/0.9 `PRI` request. In a tunnel the
    /// `upgrade` listener destroys the socket,
    /// and the plain proxy, with no listener, ends it as unanswered.
    #[error("upgrade request refused")]
    Upgrade,
    #[error("malformed request ({0})")]
    MalformedRequest(&'static str),
    #[error("malformed request-target")]
    MalformedTarget,
    #[error("malformed request-target: it holds bytes that are not UTF-8, or U+FFFD")]
    TargetBytes,
    #[error(
        "ambiguous request-target: as spelled and normalized it names different paths (a dot segment, leading slashes or \"\\\"); nothing was forwarded (network.ambiguousRequestTarget in the sandbox's configuration is \"refuse\")"
    )]
    AmbiguousTarget,
    #[error("missing Host header")]
    MissingHost,
    #[error("{0}")]
    HostMismatch(HostMismatchError),
    #[error("request denied by the request rules of the network policy")]
    RequestDenied,
    #[error("request body exceeds the size limit")]
    BodyTooLarge,
    #[error("{0}")]
    Sigv4(Sigv4RefusalError),
}

impl RefusalError {
    pub(crate) fn unparsed(error: HeadParseError) -> Self {
        match error {
            HeadParseError::InvalidUrl => RefusalError::UnparsedTarget,
            error => RefusalError::Unparsed(error),
        }
    }

    pub fn status(&self) -> StatusCode {
        match self {
            RefusalError::EchoMethod => StatusCode::METHOD_NOT_ALLOWED,
            RefusalError::Unparsed(_)
            | RefusalError::UnparsedBody(_)
            | RefusalError::UnparsedTarget
            | RefusalError::Upgrade
            | RefusalError::MalformedRequest(_)
            | RefusalError::MalformedTarget
            | RefusalError::TargetBytes
            | RefusalError::AmbiguousTarget
            | RefusalError::MissingHost => StatusCode::BAD_REQUEST,
            RefusalError::HostMismatch(_) => StatusCode::MISDIRECTED_REQUEST,
            RefusalError::RequestDenied => StatusCode::FORBIDDEN,
            RefusalError::BodyTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            RefusalError::Sigv4(refusal) => refusal.status(),
        }
    }

    pub(crate) fn tag(&self) -> &'static str {
        match self {
            RefusalError::EchoMethod => "method-refused",
            RefusalError::Unparsed(_)
            | RefusalError::UnparsedBody(_)
            | RefusalError::UnparsedTarget
            | RefusalError::Upgrade
            | RefusalError::MalformedRequest(_) => "malformed-request",
            RefusalError::MalformedTarget | RefusalError::TargetBytes => "malformed-target",
            RefusalError::AmbiguousTarget => "ambiguous-request-target",
            RefusalError::MissingHost => "missing-host",
            RefusalError::HostMismatch(_) => "host-mismatch",
            RefusalError::RequestDenied => "request-denied",
            RefusalError::BodyTooLarge => "body-too-large",
            RefusalError::Sigv4(_) => "sigv4-refused",
        }
    }

    /// Whether the proxy leaves the tunnel open after answering this refusal: a
    /// request for another host is answered and the tunnel goes on to the
    /// next request, where every other
    /// refusal ends it.
    /// a strict HTTP server ends the connection of a request without a `Host` before
    /// the proxy sees it.
    pub(crate) fn keeps_tunnel(&self) -> bool {
        matches!(self, RefusalError::HostMismatch(_))
    }

    /// Whether the proxy ends the connection with nothing written.
    pub(crate) fn ends_unanswered(&self) -> bool {
        matches!(self, RefusalError::UnparsedBody(_) | RefusalError::Upgrade)
    }

    /// The answer the proxy writes for this refusal at `at`. A head a strict HTTP parser
    /// refused is answered on the raw socket,
    /// a missing `Host` by a strict HTTP server itself, and the
    /// rest through a strict HTTP server.
    pub(crate) fn answer(&self, request: AnsweredRequest, at: SystemTime) -> Vec<u8> {
        let mark = crate::deny_header_style::header_field(self.tag());
        let status = self.status().as_u16();
        match self {
            RefusalError::MissingHost => FramedAnswer::new(
                FramedHead::new(status, vec![("Connection".to_string(), "close".to_string())]),
                "",
            )
            .bytes(request, at),
            RefusalError::RequestDenied => RequestDenial::new().answer(request, at),
            RefusalError::UnparsedBody(_) | RefusalError::Upgrade => Vec::new(),
            RefusalError::Unparsed(_) | RefusalError::UnparsedTarget | RefusalError::MalformedRequest(_) => {
                let body = format!("{self}\n");
                let mark = mark.map_or_else(String::new, |(name, value)| format!("{name}: {value}\r\n"));
                format!(
                    "HTTP/1.1 {status} {}\r\nContent-Type: text/plain\r\n{mark}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    status_text(status),
                    body.len()
                )
                .into_bytes()
            }
            _ => self.framed_answer().bytes(request, at),
        }
    }

    /// The answer a strict HTTP server writes for this refusal.
    pub(crate) fn framed_answer(&self) -> FramedAnswer {
        FramedAnswer::plain(
            self.status().as_u16(),
            crate::deny_header_style::header_field(self.tag()),
            format!("{self}\n"),
        )
    }
}

#[cfg(test)]
mod tests;
