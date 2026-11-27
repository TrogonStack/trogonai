//! The request line as a strict HTTP/1 parser reads it, which is what the proxy's
//! server sees before any of its own code runs: the method is matched against
//! the method table, a line without a protocol is an HTTP/0.9 request,
//! and the protocol may be `HTTP/`, `RTSP/` or `ICE/` with a version of 0.9,
//! 1.0, 1.1 or 2.0. Every version but x.1 is answered the way HTTP/1.0 is.
//! the strict parser also knows `PRI`, the start of the HTTP/2 connection preface.

use crate::head_parse_error::HeadParseError;
use crate::request_version::RequestVersion;

mod constants;
pub(crate) use constants::PRI;
use constants::{HTTP_METHODS, PREFACE_REST, RTSP_ONLY_METHODS, RTSP_SHARED_METHODS};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Protocol {
    Http,
    Rtsp,
    Ice,
}

impl Protocol {
    fn read(rest: &str) -> Option<(Self, &str)> {
        [("HTTP/", Self::Http), ("RTSP/", Self::Rtsp), ("ICE/", Self::Ice)]
            .into_iter()
            .find_map(|(prefix, protocol)| rest.strip_prefix(prefix).map(|rest| (protocol, rest)))
    }

    fn admits(self, method: &str) -> Result<(), HeadParseError> {
        match self {
            Self::Http if HTTP_METHODS.contains(&method) || method == PRI => Ok(()),
            Self::Http => Err(HeadParseError::InvalidMethodForHttp),
            Self::Rtsp if RTSP_SHARED_METHODS.contains(&method) || RTSP_ONLY_METHODS.contains(&method) => Ok(()),
            Self::Rtsp => Err(HeadParseError::InvalidMethodForRtsp),
            Self::Ice if method == "SOURCE" => Ok(()),
            Self::Ice => Err(HeadParseError::ExpectedSourceForIce),
        }
    }
}

/// How the line ended: with CRLF, or with a line feed alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LineEnd {
    Crlf,
    BareLf,
}

/// Whether the line names its protocol, or is an HTTP/0.9 simple request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LineForm {
    Simple,
    Full,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RequestLine<'a> {
    pub(crate) method: &'a str,
    pub(crate) target: &'a str,
    pub(crate) version: RequestVersion,
    pub(crate) form: LineForm,
}

/// What the strict parser makes of a request that opens with a `PRI` line naming a
/// protocol: it reads the rest of the HTTP/2 connection preface right after
/// the version and stops there, whatever the preface holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Preface {
    Partial,
    Refused(HeadParseError),
}

impl Preface {
    /// The preface read of the request at the start of `buf`, when its line
    /// is a `PRI` line naming a protocol.
    pub(crate) fn of(buf: &[u8]) -> Option<Self> {
        let line_end = buf.iter().position(|&byte| byte == b'\n')?;
        let (line, end) = match buf[..line_end].strip_suffix(b"\r") {
            Some(line) => (line, LineEnd::Crlf),
            None => (&buf[..line_end], LineEnd::BareLf),
        };
        match RequestLine::read(std::str::from_utf8(line).ok()?, end) {
            Ok(RequestLine {
                method: PRI,
                form: LineForm::Full,
                ..
            }) => {}
            Err(HeadParseError::ExpectedPreface) => {
                return Some(Self::Refused(HeadParseError::ExpectedPreface));
            }
            _ => return None,
        }
        let rest = &buf[line.len()..];
        Some(if rest.starts_with(PREFACE_REST) {
            Self::Refused(HeadParseError::PausedUpgrade)
        } else if PREFACE_REST.starts_with(rest) {
            Self::Partial
        } else {
            Self::Refused(HeadParseError::ExpectedPreface)
        })
    }
}

impl<'a> RequestLine<'a> {
    /// Reads `line`, the request line without its line ending.
    pub(crate) fn read(line: &'a str, end: LineEnd) -> Result<Self, HeadParseError> {
        let method = read_method(line)?;
        let rest = line[method.len()..].trim_start_matches(' ');
        let target_end = rest.find(' ').unwrap_or(rest.len());
        let target = &rest[..target_end];
        if target.is_empty() || target.contains('\r') {
            return Err(HeadParseError::InvalidUrl);
        }
        let rest = &rest[target_end..];
        if rest.is_empty() {
            return Ok(Self {
                method,
                target,
                version: RequestVersion::Http10,
                form: LineForm::Simple,
            });
        }
        let (protocol, digits) =
            Protocol::read(rest.trim_start_matches(' ')).ok_or(HeadParseError::ExpectedProtocol)?;
        protocol.admits(method)?;
        let mut digits = digits.chars();
        let major = digits
            .next()
            .and_then(|digit| digit.to_digit(10))
            .ok_or(HeadParseError::InvalidMajorVersion)?;
        if digits.next() != Some('.') {
            return Err(HeadParseError::ExpectedDot);
        }
        let minor = digits
            .next()
            .and_then(|digit| digit.to_digit(10))
            .ok_or(HeadParseError::InvalidMinorVersion)?;
        let version = match (major, minor) {
            (1, 1) => RequestVersion::Http11,
            (0, 9) | (1, 0) | (2, 0) => RequestVersion::Http10,
            _ => return Err(HeadParseError::InvalidVersion),
        };
        if !digits.as_str().is_empty() || end == LineEnd::BareLf {
            return Err(if method == PRI {
                HeadParseError::ExpectedPreface
            } else {
                HeadParseError::ExpectedCrlf
            });
        }
        Ok(Self {
            method,
            target,
            version,
            form: LineForm::Full,
        })
    }

    /// The request line that opens `head`, and the rest of the head after its
    /// line ending.
    pub(crate) fn split(head: &str) -> (&str, LineEnd, &str) {
        match head.split_once('\n') {
            Some((line, rest)) => match line.strip_suffix('\r') {
                Some(line) => (line, LineEnd::Crlf, rest),
                None => (line, LineEnd::BareLf, rest),
            },
            None => (head, LineEnd::Crlf, ""),
        }
    }
}

/// The method at the start of `line`, matched the way the parser walks its method
/// table: byte by byte, with no going back once a byte leaves every method.
fn read_method(line: &str) -> Result<&str, HeadParseError> {
    let known = || {
        HTTP_METHODS
            .iter()
            .chain(RTSP_ONLY_METHODS.iter())
            .chain(std::iter::once(&PRI))
    };
    let walked = (1..=line.len())
        .take_while(|&end| known().any(|method| method.as_bytes().starts_with(&line.as_bytes()[..end])))
        .last()
        .unwrap_or(0);
    let method = &line[..walked];
    if !known().any(|known| *known == method) {
        return Err(HeadParseError::InvalidMethod);
    }
    if !line[walked..].starts_with(' ') {
        return Err(HeadParseError::ExpectedSpaceAfterMethod);
    }
    Ok(method)
}

#[cfg(test)]
mod tests;
