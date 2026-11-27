//! Responses framed as a strict HTTP server frames them, which is how the proxy's
//! answers inside a terminated tunnel reach the client: the given fields are
//! written in order, then `Date`, the connection lines and, for a body of
//! unknown length, `Transfer-Encoding: chunked` are added.

use std::time::{SystemTime, UNIX_EPOCH};

use crate::connection_headers::{asks_to_close, asks_to_keep_alive};
use crate::request_line::RequestLine;
use crate::request_version::RequestVersion;

mod constants;
use constants::FIRST_VALUE_WINS;

/// What a strict HTTP server knows of the request it answers that changes the
/// framing of the answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AnsweredRequest {
    head: bool,
    keep_alive: bool,
    version: RequestVersion,
}

impl AnsweredRequest {
    /// An HTTP/1.1 request with `method`, which kept its connection unless
    /// `closes`.
    pub(crate) fn new(method: &str, closes: bool) -> Self {
        Self::of_version(method, RequestVersion::Http11, closes)
    }

    /// A request with `method` in `version`, which kept its connection
    /// unless `closes`.
    pub(crate) fn of_version(method: &str, version: RequestVersion, closes: bool) -> Self {
        Self {
            head: method.eq_ignore_ascii_case("HEAD"),
            keep_alive: !closes,
            version,
        }
    }

    /// The request a head names, read loosely: one that cannot be read is
    /// answered as an HTTP/1.1 request that closes.
    pub(crate) fn of_head(head: &[u8]) -> Self {
        let Ok(text) = std::str::from_utf8(head) else {
            return Self::new("GET", true);
        };
        let (request_line, end, rest) = RequestLine::split(text);
        let Ok(RequestLine { method, version, .. }) = RequestLine::read(request_line, end) else {
            return Self::new(request_line.split(' ').next().unwrap_or_default(), true);
        };
        let fields: Vec<(String, String)> = rest
            .split("\r\n")
            .filter_map(|line| line.split_once(':'))
            .map(|(name, value)| (name.trim().to_string(), value.trim().to_string()))
            .collect();
        let keep_alive = match version {
            RequestVersion::Http11 => !asks_to_close(&fields),
            RequestVersion::Http10 => asks_to_keep_alive(&fields),
        };
        Self {
            head: method.eq_ignore_ascii_case("HEAD"),
            keep_alive,
            version,
        }
    }

    /// Whether the client asked to keep its connection after this request.
    pub(crate) fn keeps_alive(&self) -> bool {
        self.keep_alive
    }

    pub(crate) fn version(&self) -> RequestVersion {
        self.version
    }

    /// The same request answered on a connection that ends after it.
    pub(crate) fn closing(self) -> Self {
        Self {
            keep_alive: false,
            ..self
        }
    }
}

/// A status line and the fields given, in their order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FramedHead {
    status: u16,
    fields: Vec<(String, String)>,
}

impl FramedHead {
    pub(crate) fn new(status: u16, fields: Vec<(String, String)>) -> Self {
        Self { status, fields }
    }

    fn has(&self, name: &str) -> bool {
        self.fields.iter().any(|(field, _)| field.eq_ignore_ascii_case(name))
    }

    fn has_body(&self, request: AnsweredRequest) -> bool {
        !request.head && !matches!(self.status, 100..=199 | 204 | 304)
    }

    /// Whether the body that follows this head goes out in chunks.
    pub(crate) fn chunks(&self, request: AnsweredRequest) -> bool {
        request.version == RequestVersion::Http11
            && self.has_body(request)
            && !self.has("content-length")
            && !self.has("transfer-encoding")
    }

    /// Whether the workload's HTTP stack keeps the connection after writing this head to
    /// `request`: the request asked to, and the head gives a length or the
    /// request is HTTP/1.1, whose answers the workload's HTTP stack chunks by default.
    pub(crate) fn keeps_alive(&self, request: AnsweredRequest) -> bool {
        request.keep_alive && (request.version == RequestVersion::Http11 || self.has("content-length"))
    }

    /// The head as the workload's HTTP stack writes it to `request` at `at`.
    pub(crate) fn render(&self, request: AnsweredRequest, at: SystemTime) -> String {
        let mut out = format!("HTTP/1.1 {} {}\r\n", self.status, status_text(self.status));
        for (name, value) in &self.fields {
            out.push_str(&format!("{name}: {value}\r\n"));
        }
        if !self.has("date") {
            out.push_str(&format!("Date: {}\r\n", http_date(at)));
        }
        if !self.has("connection") {
            if self.keeps_alive(request) {
                out.push_str("Connection: keep-alive\r\n");
                if !self.has("keep-alive") {
                    out.push_str(&format!(
                        "Keep-Alive: timeout={}\r\n",
                        crate::constants::KEEP_ALIVE_TIMEOUT.as_secs()
                    ));
                }
            } else {
                out.push_str("Connection: close\r\n");
            }
        }
        if self.chunks(request) {
            out.push_str("Transfer-Encoding: chunked\r\n");
        }
        out.push_str("\r\n");
        out
    }
}

/// A whole answer a strict HTTP server writes with a body after its head.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FramedAnswer {
    head: FramedHead,
    body: String,
}

impl FramedAnswer {
    pub(crate) fn new(head: FramedHead, body: impl Into<String>) -> Self {
        Self {
            head,
            body: body.into(),
        }
    }

    /// The answer's `text/plain` form: the content type, then the mark
    /// naming the refusal when there is one.
    pub(crate) fn plain(status: u16, mark: Option<(String, String)>, body: String) -> Self {
        let mut fields = vec![("Content-Type".to_string(), "text/plain".to_string())];
        fields.extend(mark);
        Self::new(FramedHead::new(status, fields), body)
    }

    pub(crate) fn keeps_alive(&self, request: AnsweredRequest) -> bool {
        self.head.keeps_alive(request)
    }

    pub(crate) fn bytes(&self, request: AnsweredRequest, at: SystemTime) -> Vec<u8> {
        let mut out = self.head.render(request, at);
        if self.head.chunks(request) {
            if !self.body.is_empty() {
                out.push_str(&format!("{:x}\r\n{}\r\n", self.body.len(), self.body));
            }
            out.push_str("0\r\n\r\n");
        } else if self.head.has_body(request) {
            out.push_str(&self.body);
        }
        out.into_bytes()
    }
}

/// The reason phrase the workload's HTTP stack writes for `status`, which
/// is `unknown` for a status it does not list.
pub(crate) fn status_text(status: u16) -> &'static str {
    match status {
        100 => "Continue",
        101 => "Switching Protocols",
        102 => "Processing",
        103 => "Early Hints",
        200 => "OK",
        201 => "Created",
        202 => "Accepted",
        203 => "Non-Authoritative Information",
        204 => "No Content",
        205 => "Reset Content",
        206 => "Partial Content",
        207 => "Multi-Status",
        208 => "Already Reported",
        226 => "IM Used",
        300 => "Multiple Choices",
        301 => "Moved Permanently",
        302 => "Found",
        303 => "See Other",
        304 => "Not Modified",
        305 => "Use Proxy",
        307 => "Temporary Redirect",
        308 => "Permanent Redirect",
        400 => "Bad Request",
        401 => "Unauthorized",
        402 => "Payment Required",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        406 => "Not Acceptable",
        407 => "Proxy Authentication Required",
        408 => "Request Timeout",
        409 => "Conflict",
        410 => "Gone",
        411 => "Length Required",
        412 => "Precondition Failed",
        413 => "Payload Too Large",
        414 => "URI Too Long",
        415 => "Unsupported Media Type",
        416 => "Range Not Satisfiable",
        417 => "Expectation Failed",
        418 => "I'm a Teapot",
        421 => "Misdirected Request",
        422 => "Unprocessable Entity",
        423 => "Locked",
        424 => "Failed Dependency",
        425 => "Too Early",
        426 => "Upgrade Required",
        428 => "Precondition Required",
        429 => "Too Many Requests",
        431 => "Request Header Fields Too Large",
        451 => "Unavailable For Legal Reasons",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        505 => "HTTP Version Not Supported",
        506 => "Variant Also Negotiates",
        507 => "Insufficient Storage",
        508 => "Loop Detected",
        509 => "Bandwidth Limit Exceeded",
        510 => "Not Extended",
        511 => "Network Authentication Required",
        _ => "unknown",
    }
}

/// The fields of a message as the parsed header map holds them and
/// writes them again: names lowercased, a repeated field folded into its
/// first place (`cookie` with `; `, `set-cookie` lines kept apart but side by
/// side, the fields in [`FIRST_VALUE_WINS`] keeping the first, the rest
/// joined with `, `).
pub(crate) fn parsed_fields(fields: &[(String, String)]) -> Vec<(String, String)> {
    let mut view: Vec<(String, String)> = Vec::with_capacity(fields.len());
    for (name, value) in fields {
        let name = name.to_ascii_lowercase();
        let known = view.iter_mut().find(|(seen, _)| *seen == name);
        match known {
            None => view.push((name, value.clone())),
            Some(_) if name == "set-cookie" => {
                let after = view
                    .iter()
                    .rposition(|(seen, _)| *seen == name)
                    .map_or(view.len(), |last| last + 1);
                view.insert(after, (name, value.clone()));
            }
            Some(_) if FIRST_VALUE_WINS.contains(&name.as_str()) => {}
            Some((_, joined)) => {
                joined.push_str(if name == "cookie" { "; " } else { ", " });
                joined.push_str(value);
            }
        }
    }
    view
}

/// `at` in the IMF-fixdate form of the `Date` field.
pub(crate) fn http_date(at: SystemTime) -> String {
    const DAYS: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let seconds = at.duration_since(UNIX_EPOCH).map_or(0, |elapsed| elapsed.as_secs());
    let days = seconds / 86_400;
    let rest = seconds % 86_400;
    let shifted = days + 719_468;
    let era = shifted / 146_097;
    let day_of_era = shifted % 146_097;
    let year_of_era = (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + u64::from(month <= 2);
    format!(
        "{}, {day:02} {} {year} {:02}:{:02}:{:02} GMT",
        DAYS[(days % 7) as usize],
        MONTHS[(month - 1) as usize],
        rest / 3_600,
        rest % 3_600 / 60,
        rest % 60
    )
}

/// `answer` with the value of every `Date` field, which names the moment it
/// was written, replaced by `<DATE>`.
#[cfg(test)]
pub(crate) fn undated(answer: &str) -> String {
    answer
        .split_inclusive("\r\n")
        .map(|line| match line.split_at_checked(6) {
            Some((name, _)) if name.eq_ignore_ascii_case("date: ") && line.ends_with(" GMT\r\n") => {
                format!("{name}<DATE>\r\n")
            }
            _ => line.to_string(),
        })
        .collect()
}

#[cfg(test)]
mod tests;
