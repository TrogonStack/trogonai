//! The two checks the upstream bridge runs before it trusts a repository address.
//!
//! One refuses bracketed (IPv6 style) and `file` forms. The other refuses an address whose host is read differently by
//! the ways a program, a Git client or a browser might read it: it parses the address several ways and rejects it when
//! the canonical hosts differ.

use url::Url;

use crate::bridge_unicode::{is_space, js_lowercase};

pub(crate) mod constants;

use constants::{GITHUB, SPECIAL_SCHEMES};

/// Whether an address is refused outright; `text` is already trimmed.
pub(crate) fn rejected(text: &str) -> bool {
    bracket_form(text) || host_differential(text)
}

/// The scheme and the text after `://` when `text` starts with a scheme.
fn split_scheme(text: &str, digit_first: bool) -> Option<(&str, &str)> {
    let first = text.chars().next()?;
    if !(first.is_ascii_alphabetic() || (digit_first && first.is_ascii_digit())) {
        return None;
    }
    let end = text
        .find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-')))
        .unwrap_or(text.len());
    text[end..].strip_prefix("://").map(|after| (&text[..end], after))
}

/// The hostname the URL standard reads out of `input`, when there is one.
fn hostname(input: &str) -> Option<String> {
    Url::parse(input).ok().as_ref().and_then(url_hostname)
}

fn url_hostname(url: &Url) -> Option<String> {
    url.host_str().filter(|host| !host.is_empty()).map(str::to_owned)
}

pub(crate) fn hex_pair(bytes: &[u8], at: usize) -> Option<u8> {
    let digit = |offset: usize| {
        bytes
            .get(at + offset)
            .and_then(|byte| char::from(*byte).to_digit(16))
            .and_then(|digit| u8::try_from(digit).ok())
    };
    if bytes.get(at) != Some(&b'%') {
        return None;
    }
    Some(digit(1)? * 16 + digit(2)?)
}

/// Replaces each `%XX` but `%00` with the character of that code, one byte at a time.
fn decode_latin1(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let (mut at, mut copied) = (0, 0);
    while at < bytes.len() {
        if let Some(byte) = hex_pair(bytes, at).filter(|byte| *byte != 0) {
            out.push_str(&text[copied..at]);
            out.push(char::from(byte));
            at += 3;
            copied = at;
        } else {
            at += 1;
        }
    }
    out.push_str(&text[copied..]);
    out
}

/// Decodes each run of `%XX` escapes (not `%00`) as UTF-8, or one byte at a time when the run is not valid UTF-8.
fn decode_uri_runs(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let (mut at, mut copied) = (0, 0);
    while at < bytes.len() {
        if hex_pair(bytes, at).is_none_or(|byte| byte == 0) {
            at += 1;
            continue;
        }
        let start = at;
        let mut raw = Vec::new();
        while let Some(byte) = hex_pair(bytes, at).filter(|byte| *byte != 0) {
            raw.push(byte);
            at += 3;
        }
        out.push_str(&text[copied..start]);
        match String::from_utf8(raw) {
            Ok(decoded) => out.push_str(&decoded),
            Err(_) => out.push_str(&decode_latin1(&text[start..at])),
        }
        copied = at;
    }
    out.push_str(&text[copied..]);
    out
}

fn drive_letter(text: &str) -> bool {
    let mut chars = text.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic()) && chars.next() == Some(':')
}

/// The `[...]` at `at` made only of hexadecimal digits, colons and dots, and where it ends.
fn bracket_at(text: &str, at: usize) -> Option<(&str, usize)> {
    let inner = text.get(at..)?.strip_prefix('[')?;
    let len = inner
        .bytes()
        .take_while(|byte| byte.is_ascii_hexdigit() || matches!(byte, b':' | b'.'))
        .count();
    (len > 0 && inner[len..].starts_with(']')).then(|| (&text[at..at + len + 2], at + len + 2))
}

/// `[user@][v6][:port]` filling a URL authority up to a slash or the end.
fn url_bracket_prefix(text: &str) -> Option<(&str, usize)> {
    let user_end = text.find(['@', '/', '?', '#', '\\', '[', ']']);
    let start = user_end
        .filter(|end| *end > 0 && text[*end..].starts_with('@'))
        .map_or(0, |end| end + 1);
    let (host, mut end) = bracket_at(text, start)?;
    if let Some(after) = text[end..].strip_prefix(':') {
        end += 1 + after.bytes().take_while(u8::is_ascii_digit).count();
    }
    (text[end..].is_empty() || text[end..].starts_with('/')).then_some((host, end))
}

/// `user@[v6]` directly followed by a colon, the shape of a bracketed `scp` style address.
fn scp_bracket_prefix(text: &str) -> Option<(&str, usize)> {
    let user_end = text.find(['@', ':', '/', '[', ']'])?;
    if user_end == 0 || !text[user_end..].starts_with('@') {
        return None;
    }
    let (host, end) = bracket_at(text, user_end + 1)?;
    text[end..].starts_with(':').then_some((host, end))
}

/// Refuses `file` addresses and any address with a bracket that is not a well formed IPv6 host.
pub(crate) fn bracket_form(text: &str) -> bool {
    let trimmed = text.trim_start_matches(|c: char| u32::from(c) <= 32);
    let scheme = split_scheme(trimmed, true);
    let rest = scheme.map_or(trimmed, |(_, after)| after);
    let colon = rest.find(':');
    let scp_like = colon.is_some_and(|at| rest.find('/').is_none_or(|slash| at < slash)) && !drive_letter(rest);
    let applies = match scheme {
        Some((name, _)) => !name.eq_ignore_ascii_case("file"),
        None => scp_like,
    };
    if !applies {
        let flattened = if scheme.is_some() {
            decode_latin1(rest)
        } else {
            rest.to_owned()
        };
        return flattened.contains("@[") || flattened.starts_with('[');
    }
    let decoded = decode_latin1(rest);
    let prefix = if scheme.is_some() {
        url_bracket_prefix(&decoded)
    } else {
        scp_bracket_prefix(&decoded)
    };
    let from = prefix
        .filter(|(host, _)| hostname(&format!("ssh://{host}/")).is_some())
        .map_or(0, |(_, end)| end);
    decoded[from..].contains(['[', ']'])
}

/// The port text `Pk` drops when it is a number in range, with the text before the first colon.
fn without_port(text: &str) -> &str {
    let Some(colon) = text.find(':') else {
        return text;
    };
    let tail = text[colon + 1..].trim_start_matches(['\t', '\n', '\u{b}', '\u{c}', '\r', ' ']);
    let negative = tail.starts_with('-');
    let digits = tail.strip_prefix(['+', '-']).unwrap_or(tail);
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return text;
    }
    let magnitude = digits.trim_start_matches('0');
    let in_range =
        magnitude.len() <= 5 && magnitude.parse::<u32>().unwrap_or(0) < 65536 && (!negative || magnitude.is_empty());
    if in_range { &text[..colon] } else { text }
}

fn after_last_at(text: &str) -> &str {
    text.rsplit_once('@').map_or(text, |(_, rest)| rest)
}

/// The host and path of an address split at the first `separator` (`:` for `scp` style, `/` for URLs) outside
/// brackets; the path keeps the slash a URL separator starts it with.
fn split_address(text: &str, separator: char) -> Option<(String, String)> {
    let open = text.find("@[").map_or(0, |at| at + 1);
    let open = text[open..].starts_with('[').then_some(open);
    let close = open.and_then(|open| text[open + 1..].find(']').map(|at| open + 1 + at));
    let search_from = close.map_or(0, |close| close + 1);
    let cut = text[search_from..].find(separator).map(|at| search_from + at);
    if separator == ':'
        && (cut.is_none()
            || text
                .find('/')
                .is_some_and(|slash| text.find(':').is_some_and(|colon| slash < colon)))
    {
        return None;
    }
    let path = cut
        .map_or("", |cut| &text[if separator == ':' { cut + 1 } else { cut }..])
        .to_owned();
    if let (Some(open), Some(close)) = (open, close) {
        let port = &text[close + 1..cut.unwrap_or(text.len())];
        let inside = format!("{}{}", &text[..open], &text[open + 1..close]);
        let merged = if without_port(port) == port {
            without_port(&inside).to_owned()
        } else {
            inside
        };
        let host = after_last_at(&merged);
        let parsed = hostname(&format!("ssh://[{host}]/"));
        return (!host.is_empty()).then(|| (parsed.unwrap_or_else(|| host.to_owned()), path));
    }
    let authority = cut.map_or(text, |cut| &text[..cut]);
    let host = after_last_at(authority);
    if host.starts_with('[') {
        return None;
    }
    let host = match host.rsplit_once(':') {
        Some((before, port)) if separator == '/' && port.bytes().all(|byte| byte.is_ascii_digit()) => before,
        _ => host,
    };
    (!host.is_empty()).then(|| (host.to_owned(), path))
}

/// The host and path of an address read the way a Git client would (`Ul`).
pub(crate) fn git_address(text: &str, rewritten: bool) -> Option<(String, String)> {
    let trimmed = text.trim_start_matches(is_space);
    match split_scheme(trimmed, false) {
        Some((scheme, _)) if scheme.eq_ignore_ascii_case("file") => None,
        Some((scheme, _)) if scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https") => {
            let url = Url::parse(trimmed).ok()?;
            url_hostname(&url).map(|host| (host, url.path().to_owned()))
        }
        Some((_, after)) => split_address(&decode_uri_runs(after), '/'),
        None if rewritten => split_address(&decode_uri_runs(trimmed), ':'),
        None => split_address(trimmed, ':'),
    }
}

/// The host of an address read the way a Git client would.
fn git_host(text: &str, rewritten: bool) -> Option<String> {
    git_address(text, rewritten).map(|(host, _)| host)
}

/// The address parsed after decoding the escapes in everything but web schemes (`x6`).
pub(crate) fn rewritten_url(text: &str) -> Option<Url> {
    let trimmed = text.trim_start_matches(is_space);
    match split_scheme(trimmed, false) {
        Some((scheme, after)) if !(scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https")) => {
            Url::parse(&format!("{scheme}://{}", decode_uri_runs(after))).ok()
        }
        _ => Url::parse(trimmed).ok(),
    }
}

/// The host of an address after decoding the escapes in everything but web schemes.
fn decoded_url_host(text: &str) -> Option<String> {
    rewritten_url(text).as_ref().and_then(url_hostname)
}

fn trim_trailing_dots(text: &str) -> &str {
    text.trim_end_matches('.')
}

/// A host lower cased, stripped of a trailing dot and normalized the way the URL standard normalizes a host.
pub(crate) fn fold_host(host: &str) -> String {
    let cleaned: String = host.chars().filter(|c| !matches!(c, '\t' | '\n' | '\r')).collect();
    let lowered = trim_trailing_dots(&js_lowercase(&cleaned)).to_owned();
    if lowered.is_empty()
        || lowered
            .chars()
            .any(|c| matches!(c, ':' | '/' | '\\' | '?' | '#' | '@') || is_space(c))
    {
        return lowered;
    }
    let Ok(url) = Url::parse(&format!("https://{lowered}")) else {
        return lowered;
    };
    if !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return lowered;
    }
    trim_trailing_dots(url.host_str().unwrap_or_default()).to_owned()
}

pub(crate) fn is_github(host: &str) -> bool {
    let mut folded = fold_host(host);
    while let Some(rest) = folded.strip_prefix("www.") {
        folded = rest.to_owned();
    }
    folded == GITHUB
}

pub(crate) fn canonical_host(host: &str) -> String {
    let folded = fold_host(host);
    if is_github(&folded) { GITHUB.to_owned() } else { folded }
}

/// Whether a backslash can move the end of the authority, which the web schemes and Git read differently.
pub(crate) fn backslash_authority(text: &str) -> bool {
    let cleaned: String = text
        .trim_start_matches(|c: char| u32::from(c) <= 0x20)
        .chars()
        .filter(|c| !matches!(c, '\t' | '\n' | '\r'))
        .collect();
    let Some(marker) = cleaned.find("://") else {
        return false;
    };
    let mut after = &cleaned[marker + 3..];
    if SPECIAL_SCHEMES.contains(&js_lowercase(&cleaned[..marker]).as_str()) {
        let slashes = after.len() - after.trim_start_matches(['/', '\\']).len();
        if after[..slashes].contains('\\') {
            return true;
        }
        after = &after[slashes..];
    }
    let end = after.find(['/', '?', '#']).unwrap_or(after.len());
    after[..end].contains('\\')
}

/// Refuses an address with an escaped NUL, a backslash in its authority or hosts that differ between readings.
fn host_differential(text: &str) -> bool {
    if text.contains("%00") || text.contains('\0') || backslash_authority(text) {
        return true;
    }
    let mut hosts = vec![git_host(text, false), git_host(text, true)];
    if text.contains("://") {
        hosts.push(hostname(text.trim_start_matches(is_space)));
        hosts.push(decoded_url_host(text));
    }
    let hosts: Vec<Option<String>> = hosts
        .into_iter()
        .map(|host| host.map(|host| canonical_host(&host)))
        .collect();
    hosts.iter().any(|host| *host != hosts[0])
}

#[cfg(test)]
#[path = "bridge_url_tests.rs"]
mod tests;
