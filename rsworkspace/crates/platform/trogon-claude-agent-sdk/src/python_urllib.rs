//! Ports of CPython 3.12 `urllib.parse.urljoin`, `urldefrag` and `unquote`, as `referencing` 0.37 and `jsonschema`
//! 4.26 call them to resolve `$id` and `$ref`, with the `urlsplit`, `urlparse`, `urlunparse` and `urlunsplit` logic they
//! rely on and the `ipaddress.ip_address` validity check `urlsplit` applies to bracketed hosts.
//!
//! The `ValueError` texts CPython raises are surfaced as `Err(text)`. `urljoin` and `urldefrag` can raise (invalid
//! bracketed hosts, netlocs that change under NFKC normalization); `unquote` never raises.
//!
//! `urlsplit` also strips and de-controls its `scheme` argument, but `urljoin` only ever passes the empty string or
//! the scheme of the base URL, which holds scheme characters only, so that step cannot change anything and is not
//! ported. `allow_fragments` is always its default, true.
//!
//! `python_lower` and `python_upper` are `str.lower()` and `str.upper()` over Unicode 15.0, for the scheme, host and
//! `no_proxy` matching `urllib.request` does.
//!
//! The functions work on the encoding of their text: UTF-8 for a `str`, and the [`PythonString`] encoding, where a
//! lone surrogate takes three bytes of its own, for a Python `str` that holds one. CPython treats a surrogate here as
//! any other non-ASCII code point. Every cut and join below falls at an ASCII byte, which is a whole code point in
//! both encodings, and `unquote` only adds UTF-8 it decoded, so the result of a `str` stays UTF-8 and the result of a
//! [`PythonString`] keeps each code point whole.

use crate::constants::{CONTEXT_SINCE_15, LOWER_SINCE_15, UPPER_SINCE_15, USES_NETLOC, USES_PARAMS, USES_RELATIVE};
use crate::{python_re::py_repr, python_string::PythonString};
use std::ops::RangeInclusive;
use unicode_normalization::UnicodeNormalization;

/// Text the URL functions take: a `str`, or a [`PythonString`] that may hold lone surrogates.
pub(crate) trait UrlText {
    /// The text the functions return for this input.
    type Owned;

    /// The encoding of the text.
    fn encoded(&self) -> &[u8];

    /// The text of an encoding cut and joined as the functions do.
    fn from_encoded(bytes: Vec<u8>) -> Self::Owned;
}

impl UrlText for str {
    type Owned = String;

    fn encoded(&self) -> &[u8] {
        self.as_bytes()
    }

    /// UTF-8 cut and joined at ASCII bytes stays UTF-8, so the lossy branch is never taken.
    fn from_encoded(bytes: Vec<u8>) -> String {
        String::from_utf8(bytes).unwrap_or_else(|error| String::from_utf8_lossy(error.as_bytes()).into_owned())
    }
}

impl UrlText for String {
    type Owned = String;

    fn encoded(&self) -> &[u8] {
        self.as_bytes()
    }

    fn from_encoded(bytes: Vec<u8>) -> String {
        str::from_encoded(bytes)
    }
}

impl UrlText for PythonString {
    type Owned = PythonString;

    fn encoded(&self) -> &[u8] {
        self.as_bytes()
    }

    fn from_encoded(bytes: Vec<u8>) -> PythonString {
        PythonString::from_encoded(bytes)
    }
}

impl<T: UrlText + ?Sized> UrlText for &T {
    type Owned = T::Owned;

    fn encoded(&self) -> &[u8] {
        (**self).encoded()
    }

    fn from_encoded(bytes: Vec<u8>) -> T::Owned {
        T::from_encoded(bytes)
    }
}

/// The `ParseResult` of `urlparse`.
pub(crate) struct Parsed<T = String> {
    pub(crate) scheme: T,
    pub(crate) netloc: T,
    pub(crate) path: T,
    pub(crate) params: T,
    pub(crate) query: T,
    pub(crate) fragment: T,
}

/// The text of a `ValueError`.
fn error(text: &str) -> Vec<u8> {
    text.as_bytes().to_vec()
}

fn listed(schemes: &[&str], scheme: &[u8]) -> bool {
    schemes.iter().any(|listed| listed.as_bytes() == scheme)
}

fn find(text: &[u8], byte: u8) -> Option<usize> {
    text.iter().position(|&b| b == byte)
}

fn split_once(text: &[u8], byte: u8) -> Option<(&[u8], &[u8])> {
    let index = find(text, byte)?;
    Some((&text[..index], &text[index + 1..]))
}

fn rsplit_once(text: &[u8], byte: u8) -> Option<(&[u8], &[u8])> {
    let index = text.iter().rposition(|&b| b == byte)?;
    Some((&text[..index], &text[index + 1..]))
}

fn is_scheme_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.')
}

/// `_splitnetloc(url, 2)` for a URL starting with `//`: the text up to the first `/`, `?` or `#`, and the rest.
fn split_netloc(url: &[u8]) -> (&[u8], &[u8]) {
    let delim = url
        .get(2..)
        .and_then(|tail| tail.iter().position(|b| matches!(b, b'/' | b'?' | b'#')))
        .map_or(url.len(), |offset| offset + 2);
    (
        url.get(2..delim).unwrap_or_default(),
        url.get(delim..).unwrap_or_default(),
    )
}

/// `ipaddress.IPv4Address(text)` succeeds.
fn is_ipv4(text: &[u8]) -> bool {
    if text.is_empty() || text.contains(&b'/') {
        return false;
    }
    let octets: Vec<&[u8]> = text.split(|&b| b == b'.').collect();
    octets.len() == 4 && octets.iter().all(|octet| is_ipv4_octet(octet))
}

/// `_BaseV4._parse_octet` succeeds.
fn is_ipv4_octet(octet: &[u8]) -> bool {
    !octet.is_empty()
        && octet.iter().all(u8::is_ascii_digit)
        && octet.len() <= 3
        && (octet == b"0" || !octet.starts_with(b"0"))
        && octet.iter().fold(0_u16, |value, b| value * 10 + u16::from(b - b'0')) <= 255
}

/// `_BaseV6._parse_hextet` succeeds (an empty hextet fails in `int('', 16)`).
fn is_ipv6_hextet(hextet: &[u8]) -> bool {
    !hextet.is_empty() && hextet.iter().all(u8::is_ascii_hexdigit) && hextet.len() <= 4
}

/// `ipaddress.IPv6Address(text)` succeeds, scope id included.
fn is_ipv6(text: &[u8]) -> bool {
    if text.contains(&b'/') {
        return false;
    }
    let (address, scope) = match split_once(text, b'%') {
        Some((address, scope)) => (address, Some(scope)),
        None => (text, None),
    };
    if scope.is_some_and(|scope| scope.is_empty() || scope.contains(&b'%')) {
        return false;
    }
    // `len(ip_str)` counts code points: one per byte that does not continue a code point.
    if address.is_empty() || address.iter().filter(|&&b| b & 0xC0 != 0x80).count() > 45 {
        return false;
    }
    let mut parts: Vec<&[u8]> = address.splitn(10, |&b| b == b':').collect();
    if parts.len() < 3 {
        return false;
    }
    if parts.last().is_some_and(|last| last.contains(&b'.')) {
        if !parts.pop().is_some_and(is_ipv4) {
            return false;
        }
        parts.push(b"0");
        parts.push(b"0");
    }
    if parts.len() > 9 {
        return false;
    }
    let mut skip_index: Option<usize> = None;
    for (index, part) in parts.iter().enumerate().take(parts.len() - 1).skip(1) {
        if part.is_empty() {
            if skip_index.is_some() {
                return false;
            }
            skip_index = Some(index);
        }
    }
    let last = parts.len() - 1;
    let (parts_hi, parts_lo) = match skip_index {
        Some(skip) => {
            let mut hi = skip;
            let mut lo = parts.len() - skip - 1;
            if parts[0].is_empty() {
                hi -= 1;
                if hi != 0 {
                    return false;
                }
            }
            if parts[last].is_empty() {
                lo -= 1;
                if lo != 0 {
                    return false;
                }
            }
            if hi + lo > 7 {
                return false;
            }
            (hi, lo)
        }
        None => {
            if parts.len() != 8 || parts[0].is_empty() || parts[last].is_empty() {
                return false;
            }
            (parts.len(), 0)
        }
    };
    parts[..parts_hi].iter().all(|part| is_ipv6_hextet(part))
        && parts[parts.len() - parts_lo..].iter().all(|part| is_ipv6_hextet(part))
}

/// `_check_bracketed_host`.
fn check_bracketed_host(hostname: &[u8]) -> Result<(), Vec<u8>> {
    if let Some(rest) = hostname.strip_prefix(b"v") {
        // `\Av[a-fA-F0-9]+\..+\Z`
        let hex = rest.iter().take_while(|b| b.is_ascii_hexdigit()).count();
        let valid = hex > 0
            && rest
                .get(hex..)
                .and_then(|tail| tail.strip_prefix(b"."))
                .is_some_and(|tail| !tail.is_empty() && !tail.contains(&b'\n'));
        return if valid {
            Ok(())
        } else {
            Err(error("IPvFuture address is invalid"))
        };
    }
    if is_ipv4(hostname) {
        Err(error("An IPv4 address cannot be in brackets"))
    } else if is_ipv6(hostname) {
        Ok(())
    } else {
        let hostname = PythonString::from_encoded(hostname.to_vec());
        Err(format!("{} does not appear to be an IPv4 or IPv6 address", py_repr(&hostname)).into_bytes())
    }
}

/// `_check_bracketed_netloc`.
fn check_bracketed_netloc(netloc: &[u8]) -> Result<(), Vec<u8>> {
    let host_and_port = rsplit_once(netloc, b'@').map_or(netloc, |(_, tail)| tail);
    let hostname = match split_once(host_and_port, b'[') {
        Some((before_bracket, bracketed)) => {
            if !before_bracket.is_empty() {
                return Err(error("Invalid IPv6 URL"));
            }
            let (hostname, port) = split_once(bracketed, b']').unwrap_or((bracketed, &[]));
            if !port.is_empty() && !port.starts_with(b":") {
                return Err(error("Invalid IPv6 URL"));
            }
            hostname
        }
        None => split_once(host_and_port, b':').map_or(host_and_port, |(hostname, _)| hostname),
    };
    check_bracketed_host(hostname)
}

/// `unicodedata.normalize('NFKC', text)`. A lone surrogate has no decomposition, combines with nothing and, as a
/// starter, blocks composition across it, so the text normalizes run by run between the surrogates, which stay.
fn nfkc(text: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len());
    let mut rest = text;
    loop {
        // `ED A0`..`ED BF` starts a surrogate, which UTF-8 never encodes.
        let cut = rest
            .windows(2)
            .position(|pair| pair[0] == 0xED && pair[1] >= 0xA0)
            .unwrap_or(rest.len());
        let (run, tail) = rest.split_at(cut);
        let normalized: String = String::from_utf8_lossy(run).nfkc().collect();
        out.extend_from_slice(normalized.as_bytes());
        let Some(surrogate) = tail.get(..3) else {
            return out;
        };
        out.extend_from_slice(surrogate);
        rest = tail.get(3..).unwrap_or_default();
    }
}

/// `_checknetloc`: a non-ASCII netloc may not grow a delimiter under NFKC normalization.
fn check_netloc(netloc: &[u8]) -> Result<(), Vec<u8>> {
    if netloc.is_ascii() {
        return Ok(());
    }
    let stripped: Vec<u8> = netloc
        .iter()
        .copied()
        .filter(|b| !matches!(b, b'@' | b':' | b'#' | b'?'))
        .collect();
    let normalized = nfkc(&stripped);
    if stripped == normalized {
        return Ok(());
    }
    if normalized.iter().any(|b| matches!(b, b'/' | b'?' | b'#' | b'@' | b':')) {
        let mut text = error("netloc '");
        text.extend_from_slice(netloc);
        text.extend_from_slice(b"' contains invalid characters under NFKC normalization");
        return Err(text);
    }
    Ok(())
}

/// `urlsplit(url, scheme)` as the five components `(scheme, netloc, path, query, fragment)`.
fn urlsplit(url: &[u8], default_scheme: &[u8]) -> Result<[Vec<u8>; 5], Vec<u8>> {
    let start = url.iter().position(|&b| b > b' ').unwrap_or(url.len());
    let cleaned: Vec<u8> = url[start..]
        .iter()
        .copied()
        .filter(|b| !matches!(b, b'\t' | b'\r' | b'\n'))
        .collect();
    let mut scheme = default_scheme.to_vec();
    let mut rest: &[u8] = &cleaned;
    if let Some(colon) = find(&cleaned, b':')
        && colon > 0
        && cleaned.first().is_some_and(u8::is_ascii_alphabetic)
        && cleaned[..colon].iter().all(|&b| is_scheme_char(b))
    {
        scheme = cleaned[..colon].to_ascii_lowercase();
        rest = &cleaned[colon + 1..];
    }
    let mut netloc: &[u8] = &[];
    if rest.starts_with(b"//") {
        (netloc, rest) = split_netloc(rest);
        let (open, close) = (netloc.contains(&b'['), netloc.contains(&b']'));
        if open != close {
            return Err(error("Invalid IPv6 URL"));
        }
        if open {
            check_bracketed_netloc(netloc)?;
        }
    }
    let (rest, fragment) = split_once(rest, b'#').unwrap_or((rest, &[]));
    let (path, query) = split_once(rest, b'?').unwrap_or((rest, &[]));
    check_netloc(netloc)?;
    Ok([
        scheme,
        netloc.to_vec(),
        path.to_vec(),
        query.to_vec(),
        fragment.to_vec(),
    ])
}

/// `_splitparams`.
fn split_params(url: &[u8]) -> (&[u8], &[u8]) {
    let found = match url.iter().rposition(|&b| b == b'/') {
        Some(slash) => find(&url[slash..], b';').map(|offset| slash + offset),
        None => find(url, b';'),
    };
    match found {
        Some(index) => (&url[..index], &url[index + 1..]),
        None => (url, &[]),
    }
}

/// `urlparse(url, scheme)` on the encoding.
fn parse(url: &[u8], default_scheme: &[u8]) -> Result<Parsed<Vec<u8>>, Vec<u8>> {
    let [scheme, netloc, path, query, fragment] = urlsplit(url, default_scheme)?;
    let (path, params) = if listed(USES_PARAMS, &scheme) && path.contains(&b';') {
        let (path, params) = split_params(&path);
        (path.to_vec(), params.to_vec())
    } else {
        (path, Vec::new())
    };
    Ok(Parsed {
        scheme,
        netloc,
        path,
        params,
        query,
        fragment,
    })
}

/// `urlparse(url, scheme)`.
pub(crate) fn urlparse(url: &str, default_scheme: &str) -> Result<Parsed, String> {
    let parsed = parse(url.as_bytes(), default_scheme.as_bytes()).map_err(str::from_encoded)?;
    Ok(Parsed {
        scheme: str::from_encoded(parsed.scheme),
        netloc: str::from_encoded(parsed.netloc),
        path: str::from_encoded(parsed.path),
        params: str::from_encoded(parsed.params),
        query: str::from_encoded(parsed.query),
        fragment: str::from_encoded(parsed.fragment),
    })
}

/// `urlparse(url)` of a text that may hold lone surrogates. `Err` carries the text of the `ValueError` CPython
/// raises.
pub(crate) fn urlparse_text<T: UrlText + ?Sized>(url: &T) -> Result<Parsed<T::Owned>, T::Owned> {
    let parsed = parse(url.encoded(), b"").map_err(T::from_encoded)?;
    Ok(Parsed {
        scheme: T::from_encoded(parsed.scheme),
        netloc: T::from_encoded(parsed.netloc),
        path: T::from_encoded(parsed.path),
        params: T::from_encoded(parsed.params),
        query: T::from_encoded(parsed.query),
        fragment: T::from_encoded(parsed.fragment),
    })
}

/// `urlunsplit`.
fn urlunsplit(scheme: &[u8], netloc: &[u8], path: &[u8], query: &[u8], fragment: &[u8]) -> Vec<u8> {
    let mut url = Vec::with_capacity(scheme.len() + netloc.len() + path.len() + query.len() + fragment.len() + 6);
    if !scheme.is_empty() {
        url.extend_from_slice(scheme);
        url.push(b':');
    }
    if !netloc.is_empty() {
        url.extend_from_slice(b"//");
        url.extend_from_slice(netloc);
        if !path.is_empty() && !path.starts_with(b"/") {
            url.push(b'/');
        }
    } else if path.starts_with(b"//")
        || (!scheme.is_empty() && listed(USES_NETLOC, scheme) && (path.is_empty() || path.starts_with(b"/")))
    {
        url.extend_from_slice(b"//");
    }
    url.extend_from_slice(path);
    if !query.is_empty() {
        url.push(b'?');
        url.extend_from_slice(query);
    }
    if !fragment.is_empty() {
        url.push(b'#');
        url.extend_from_slice(fragment);
    }
    url
}

/// `urlunparse` on the encoding.
fn unparse(parsed: &Parsed<Vec<u8>>, fragment: &[u8]) -> Vec<u8> {
    let mut path = parsed.path.clone();
    if !parsed.params.is_empty() {
        path.push(b';');
        path.extend_from_slice(&parsed.params);
    }
    urlunsplit(&parsed.scheme, &parsed.netloc, &path, &parsed.query, fragment)
}

/// `urlunparse`.
pub(crate) fn urlunparse(parsed: &Parsed, fragment: &str) -> String {
    let path = if parsed.params.is_empty() {
        parsed.path.clone()
    } else {
        format!("{};{}", parsed.path, parsed.params)
    };
    str::from_encoded(urlunsplit(
        parsed.scheme.as_bytes(),
        parsed.netloc.as_bytes(),
        path.as_bytes(),
        parsed.query.as_bytes(),
        fragment.as_bytes(),
    ))
}

/// `urljoin` on the encoding.
fn join(base: &[u8], url: &[u8]) -> Result<Vec<u8>, Vec<u8>> {
    if base.is_empty() {
        return Ok(url.to_vec());
    }
    if url.is_empty() {
        return Ok(base.to_vec());
    }
    let base_parsed = parse(base, &[])?;
    let mut parsed = parse(url, &base_parsed.scheme)?;
    if parsed.scheme != base_parsed.scheme || !listed(USES_RELATIVE, &parsed.scheme) {
        return Ok(url.to_vec());
    }
    // CPython checks `scheme in uses_netloc` here, which every scheme in `uses_relative` passes.
    if !parsed.netloc.is_empty() {
        return Ok(unparse(&parsed, &parsed.fragment));
    }
    parsed.netloc = base_parsed.netloc.clone();
    if parsed.path.is_empty() && parsed.params.is_empty() {
        parsed.path = base_parsed.path.clone();
        parsed.params = base_parsed.params.clone();
        if parsed.query.is_empty() {
            parsed.query = base_parsed.query.clone();
        }
        return Ok(unparse(&parsed, &parsed.fragment));
    }
    let mut base_parts: Vec<&[u8]> = base_parsed.path.split(|&b| b == b'/').collect();
    if base_parts.last().is_some_and(|last| !last.is_empty()) {
        base_parts.pop();
    }
    let mut segments: Vec<&[u8]> = if parsed.path.starts_with(b"/") {
        parsed.path.split(|&b| b == b'/').collect()
    } else {
        let mut segments = base_parts;
        segments.extend(parsed.path.split(|&b| b == b'/'));
        if segments.len() > 2 {
            let last = segments.len() - 1;
            let middle: Vec<&[u8]> = segments[1..last].iter().copied().filter(|s| !s.is_empty()).collect();
            let tail = segments[last];
            segments.truncate(1);
            segments.extend(middle);
            segments.push(tail);
        }
        segments
    };
    let mut resolved: Vec<&[u8]> = Vec::new();
    for segment in &segments {
        match *segment {
            b".." => {
                resolved.pop();
            }
            b"." => {}
            other => resolved.push(other),
        }
    }
    if segments.pop().is_some_and(|last| matches!(last, b"." | b"..")) {
        resolved.push(b"");
    }
    let joined = resolved.join(&b'/');
    parsed.path = if joined.is_empty() { b"/".to_vec() } else { joined };
    Ok(unparse(&parsed, &parsed.fragment))
}

/// `urllib.parse.urljoin(base, url)`. `Err` carries the text of the `ValueError` CPython raises.
pub(crate) fn urljoin<T: UrlText + ?Sized>(base: &T, url: &T) -> Result<T::Owned, T::Owned> {
    join(base.encoded(), url.encoded())
        .map(T::from_encoded)
        .map_err(T::from_encoded)
}

/// `urllib.parse.urlsplit(url).geturl()`. `Err` carries the text of the `ValueError` CPython raises.
pub(crate) fn geturl<T: UrlText + ?Sized>(url: &T) -> Result<T::Owned, T::Owned> {
    let [scheme, netloc, path, query, fragment] = urlsplit(url.encoded(), &[]).map_err(T::from_encoded)?;
    Ok(T::from_encoded(urlunsplit(&scheme, &netloc, &path, &query, &fragment)))
}

/// `urllib.parse.urldefrag(url)` as `(url without fragment, fragment)`. `Err` carries the text of the `ValueError`
/// CPython raises while parsing a URL that contains `#`.
pub(crate) fn urldefrag<T: UrlText + ?Sized>(url: &T) -> Result<(T::Owned, T::Owned), T::Owned> {
    let encoded = url.encoded();
    if !encoded.contains(&b'#') {
        return Ok((T::from_encoded(encoded.to_vec()), T::from_encoded(Vec::new())));
    }
    let parsed = parse(encoded, &[]).map_err(T::from_encoded)?;
    let without = unparse(&parsed, &[]);
    Ok((T::from_encoded(without), T::from_encoded(parsed.fragment)))
}

/// `_unquote_impl` for one ASCII run, then `.decode('utf-8', 'replace')`.
fn unquote_ascii_run(run: &[u8]) -> String {
    let mut bytes: Vec<u8> = Vec::with_capacity(run.len());
    let mut bits = run.split(|&b| b == b'%');
    bytes.extend_from_slice(bits.next().unwrap_or_default());
    for item in bits {
        let decoded = match item {
            [high, low, ..] => char::from(*high)
                .to_digit(16)
                .zip(char::from(*low).to_digit(16))
                .map(|(high, low)| (high * 16 + low) as u8),
            _ => None,
        };
        match decoded {
            Some(byte) => {
                bytes.push(byte);
                bytes.extend_from_slice(item.get(2..).unwrap_or_default());
            }
            None => {
                bytes.push(b'%');
                bytes.extend_from_slice(item);
            }
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

/// `urllib.parse.unquote(string)`: `%XX` runs inside ASCII stretches decoded as UTF-8 with replacement characters;
/// non-ASCII text, lone surrogates included, passes through unchanged.
pub(crate) fn unquote<T: UrlText + ?Sized>(text: &T) -> T::Owned {
    let mut rest = text.encoded();
    if !rest.contains(&b'%') {
        return T::from_encoded(rest.to_vec());
    }
    let mut out = Vec::with_capacity(rest.len());
    while !rest.is_empty() {
        let (run, tail) = rest.split_at(rest.iter().take_while(|b| b.is_ascii()).count());
        if !run.is_empty() {
            out.extend_from_slice(unquote_ascii_run(run).as_bytes());
        }
        let (other, tail) = tail.split_at(tail.iter().take_while(|b| !b.is_ascii()).count());
        out.extend_from_slice(other);
        rest = tail;
    }
    T::from_encoded(out)
}

/// What a code point is to the `Final_Sigma` rule of `handle_capital_sigma`. A code point both cased and case
/// ignorable is skipped, so it counts as ignorable.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SigmaContext {
    Cased,
    Ignorable,
    Other,
}

fn in_ranges(ranges: &[RangeInclusive<char>], c: char) -> bool {
    ranges.iter().any(|range| range.contains(&c))
}

/// The `Cased` and `Case_Ignorable` properties of Unicode 15.0. Rust does not expose its own, so they are read through
/// the final sigma rule of `str::to_lowercase`, which is CPython's, then corrected where Unicode changed since: U+0295
/// stopped being cased and U+1171E case ignorable in 16.0.
fn sigma_context(c: char) -> SigmaContext {
    match c {
        '\u{295}' => return SigmaContext::Cased,
        '\u{1171e}' => return SigmaContext::Ignorable,
        _ if in_ranges(&CONTEXT_SINCE_15, c) => return SigmaContext::Other,
        _ => {}
    }
    let final_sigma = |text: String| text.to_lowercase().ends_with('\u{3c2}');
    if final_sigma(format!("{c}\u{3a3}")) {
        SigmaContext::Cased
    } else if final_sigma(format!("A{c}\u{3a3}")) {
        SigmaContext::Ignorable
    } else {
        SigmaContext::Other
    }
}

/// Whether the first code point past the case ignorable ones is cased.
fn cased_past_ignorable<'a>(chars: impl Iterator<Item = &'a char>) -> bool {
    chars
        .map(|&c| sigma_context(c))
        .find(|context| *context != SigmaContext::Ignorable)
        == Some(SigmaContext::Cased)
}

/// CPython 3.12's `str.lower()`: the full lowercase mapping of Unicode 15.0, with U+03A3 becoming `ς` in the
/// `Final_Sigma` context (a cased letter before it and none after it, past case ignorable ones) and `σ` elsewhere.
pub(crate) fn python_lower(text: &str) -> String {
    if text.is_ascii() {
        return text.to_ascii_lowercase();
    }
    let chars: Vec<char> = text.chars().collect();
    let mut lower = String::with_capacity(text.len());
    for (index, &c) in chars.iter().enumerate() {
        if c == '\u{3a3}' {
            let before = chars.get(..index).unwrap_or_default();
            let after = chars.get(index + 1..).unwrap_or_default();
            let last = cased_past_ignorable(before.iter().rev()) && !cased_past_ignorable(after.iter());
            lower.push(if last { '\u{3c2}' } else { '\u{3c3}' });
        } else if in_ranges(&LOWER_SINCE_15, c) {
            lower.push(c);
        } else {
            lower.extend(c.to_lowercase());
        }
    }
    lower
}

/// CPython 3.12's `str.upper()`: the full uppercase mapping of Unicode 15.0.
pub(crate) fn python_upper(text: &str) -> String {
    let mut upper = String::with_capacity(text.len());
    for c in text.chars() {
        if in_ranges(&UPPER_SINCE_15, c) {
            upper.push(c);
        } else {
            upper.extend(c.to_uppercase());
        }
    }
    upper
}

/// Where the scheme `_splittype` matches ends: the first `:`, when it comes before any `/` and is not first.
pub(crate) fn scheme_end(url: &[u8]) -> Option<usize> {
    let end = url.iter().position(|&byte| matches!(byte, b':' | b'/'))?;
    (end != 0 && url.get(end) == Some(&b':')).then_some(end)
}

/// `_splitport(host)`: the host and the digits after its last `:`, when they are all ASCII digits.
pub(crate) fn split_port<T: UrlText + ?Sized>(host: &T) -> (T::Owned, Option<String>) {
    let bytes = host.encoded();
    if let Some(colon) = bytes.iter().rposition(|&byte| byte == b':') {
        let port = bytes.get(colon + 1..).unwrap_or_default();
        if port.iter().all(u8::is_ascii_digit) {
            let name = T::from_encoded(bytes.get(..colon).unwrap_or_default().to_vec());
            return (
                name,
                (!port.is_empty()).then(|| String::from_utf8_lossy(port).into_owned()),
            );
        }
    }
    (T::from_encoded(bytes.to_vec()), None)
}

/// `_splittype(url)`: the lowercased scheme before the first `:`, when no `/` comes first.
pub(crate) fn split_type<T: UrlText + ?Sized>(url: &T) -> Option<(PythonString, T::Owned)> {
    let url = url.encoded();
    let end = scheme_end(url)?;
    let scheme = PythonString::from_encoded(url.get(..end)?.to_vec()).map_text(python_lower);
    Some((scheme, T::from_encoded(url.get(end + 1..)?.to_vec())))
}

#[cfg(test)]
#[path = "python_urllib_tests.rs"]
mod tests;
