//! How the proxy names the reason of a refusal it generates: its own
//! `X-Proxy-Error` tags, or the `X-Deny-Reason` codes the proxy-only mode
//! uses. Each server chooses its own style, which follows its connections.

use std::future::Future;

mod constants;
use constants::DEFAULT_TAG;

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum DenyHeaderStyle {
    ProxyError,
    DenyReason,
}

tokio::task_local! {
    static STYLE: DenyHeaderStyle;
}

impl DenyHeaderStyle {
    /// Runs `connection` so every refusal it generates is named in this style.
    pub(crate) fn scope<F: Future>(self, connection: F) -> impl Future<Output = F::Output> {
        STYLE.scope(self, connection)
    }

    /// The style of the connection being served, the default one outside any.
    pub fn current() -> Self {
        STYLE.try_with(|style| *style).unwrap_or(Self::ProxyError)
    }

    /// The header line, without its line break, that names `tag`, or `None`
    /// when this style carries no mark for it. the legacy style names most
    /// of its own refusals with its default tag and
    /// leaves a bare 400 or 431 of its HTTP parser unmarked
    ///. A SigV4 refusal
    /// keeps that tag under the deny header too.
    pub fn line(self, tag: &str) -> Option<String> {
        if matches!(tag, "upstream-unreachable" | "invalid-upstream-response") {
            return None;
        }
        match self {
            Self::ProxyError => {
                let shown = match tag {
                    "bad-request" => return None,
                    "address-denied" | "malformed-target" | "malformed-request" | "method-refused"
                    | "host-mismatch" | "request-denied" | "sigv4-refused" | "missing-host" => DEFAULT_TAG,
                    other => other,
                };
                Some(format!("X-Proxy-Error: {shown}"))
            }
            Self::DenyReason if tag == "sigv4-refused" => Some(format!("X-Proxy-Error: {DEFAULT_TAG}")),
            Self::DenyReason => legacy_code(tag).map(|code| format!("X-Deny-Reason: {code}")),
        }
    }

    /// The mark of a request the request rules denied, as the proxy
    /// writes it: the legacy style always names
    /// its default tag, and the deny header carries the decision's own mark
    /// or `denied`.
    pub fn denial_line(self, mark: Option<&str>) -> String {
        match self {
            Self::ProxyError => format!("X-Proxy-Error: {DEFAULT_TAG}"),
            Self::DenyReason => format!("X-Deny-Reason: {}", mark.unwrap_or("denied")),
        }
    }
}

fn legacy_code(tag: &str) -> Option<&str> {
    match tag {
        "blocked-by-allowlist" => Some("host_not_allowed"),
        "blocked-by-egress-proxy" => Some("opaque_tunnel"),
        "address-denied" => Some("address_not_allowed"),
        "bad-request"
        | "malformed-target"
        | "malformed-request"
        | "ambiguous-request-target"
        | "missing-host"
        | "body-too-large" => Some("bad_request"),
        "host-mismatch" => Some("misdirected"),
        "method-refused" => Some("method_refused"),
        "too-many-tunnels" => Some("too_many_tunnels"),
        "request-denied" => Some("denied"),
        "unauthenticated" => None,
        other => Some(other),
    }
}

/// `X-Proxy-Error: tag\r\n` in the connection's style, or nothing.
pub(crate) fn header_line(tag: &str) -> String {
    DenyHeaderStyle::current()
        .line(tag)
        .map_or_else(String::new, |line| format!("{line}\r\n"))
}

/// The mark naming `tag` in the connection's style as a name and a value.
pub(crate) fn header_field(tag: &str) -> Option<(String, String)> {
    field(&DenyHeaderStyle::current().line(tag)?)
}

/// [`DenyHeaderStyle::denial_line`] in the connection's style as a name and a
/// value.
pub(crate) fn denial_field(mark: Option<&str>) -> Option<(String, String)> {
    field(&DenyHeaderStyle::current().denial_line(mark))
}

fn field(line: &str) -> Option<(String, String)> {
    let (name, value) = line.split_once(": ")?;
    Some((name.to_string(), value.to_string()))
}

#[cfg(test)]
mod tests;
