#![cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    allow(serde_json_macro, reason = "Tests read the recorded CPython results from JSON.")
)]

use super::*;
use crate::python_json_load::python_json_loads;
use crate::python_json_value::PythonJsonValue;
use serde_json::Value;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const GOLDEN: &str = include_str!("../tests/fixtures/sdk_mcp/python_urllib.json");

// `&str` shims over the functions that work on the encoding, so the cases read as CPython's do.

fn text(bytes: Vec<u8>) -> String {
    str::from_encoded(bytes)
}

fn as_text(bytes: &[u8]) -> &str {
    std::str::from_utf8(bytes).unwrap_or_default()
}

fn is_ipv4(text: &str) -> bool {
    super::is_ipv4(text.as_bytes())
}

fn is_ipv6(text: &str) -> bool {
    super::is_ipv6(text.as_bytes())
}

fn check_bracketed_host(hostname: &str) -> Result<(), String> {
    super::check_bracketed_host(hostname.as_bytes()).map_err(text)
}

fn check_bracketed_netloc(netloc: &str) -> Result<(), String> {
    super::check_bracketed_netloc(netloc.as_bytes()).map_err(text)
}

fn check_netloc(netloc: &str) -> Result<(), String> {
    super::check_netloc(netloc.as_bytes()).map_err(text)
}

fn urlsplit(url: &str, default_scheme: &str) -> Result<[String; 5], String> {
    super::urlsplit(url.as_bytes(), default_scheme.as_bytes())
        .map(|parts| parts.map(text))
        .map_err(text)
}

fn split_params(url: &str) -> (&str, &str) {
    let (path, params) = super::split_params(url.as_bytes());
    (as_text(path), as_text(params))
}

fn urlunsplit(scheme: &str, netloc: &str, path: &str, query: &str, fragment: &str) -> String {
    text(super::urlunsplit(
        scheme.as_bytes(),
        netloc.as_bytes(),
        path.as_bytes(),
        query.as_bytes(),
        fragment.as_bytes(),
    ))
}

fn args(case: &Value) -> Vec<&str> {
    case.get("args")
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default()
}

#[test]
fn every_recorded_cpython_case_is_reproduced() -> TestResult {
    let golden: Value = serde_json::from_str(GOLDEN)?;
    let cases = golden
        .get("cases")
        .and_then(Value::as_array)
        .ok_or("fixture has no cases")?;
    assert!(cases.len() > 1000);
    let mut errors = 0;
    for case in cases {
        let name = case.get("fn").and_then(Value::as_str).ok_or("case without fn")?;
        let inputs = args(case);
        let expected_error = case.get("error").and_then(Value::as_str);
        let expected_ok = case.get("ok");
        let (actual_ok, actual_error): (Option<Value>, Option<String>) = match (name, inputs.as_slice()) {
            ("urljoin", [base, url]) => match urljoin(base, url) {
                Ok(joined) => (Some(Value::String(joined)), None),
                Err(text) => (None, Some(text)),
            },
            ("urldefrag", [url]) => match urldefrag(url) {
                Ok((without, fragment)) => (
                    Some(Value::Array(vec![Value::String(without), Value::String(fragment)])),
                    None,
                ),
                Err(text) => (None, Some(text)),
            },
            ("unquote", [text]) => (Some(Value::String(unquote(text))), None),
            _ => return Err(format!("unknown case {case}").into()),
        };
        assert_eq!(actual_error.as_deref(), expected_error, "error for {case}");
        assert_eq!(actual_ok.as_ref(), expected_ok, "result for {case}");
        errors += usize::from(expected_error.is_some());
    }
    assert!(errors > 100);
    Ok(())
}

#[test]
fn joins_follow_cpython() {
    let cases = [
        ("http://a/b/c/d;p?q", "../g", "http://a/b/g"),
        ("http://a", "?q", "http://a?q"),
        ("http:///a", "x", "http:///x"),
        ("http://a/b", "//", "http://a/b"),
        ("http://a/b/c", "g;x/y", "http://a/b/g;x/y"),
        ("http://a/b;c", "d", "http://a/d"),
        ("http://a/b?x", "#f", "http://a/b?x#f"),
        ("a/b", "../../../c", "c"),
        ("x", "y", "y"),
        ("http://a/b/c/d;p?q", "..", "http://a/b/"),
        ("", "rel", "rel"),
        ("http://a/b", "", "http://a/b"),
    ];
    for (base, url, expected) in cases {
        assert_eq!(urljoin(base, url).as_deref(), Ok(expected), "{base} + {url}");
        assert_eq!(
            urljoin(&base.to_owned(), &url.to_owned()).as_deref(),
            Ok(expected),
            "{base} + {url}"
        );
    }
}

#[test]
fn urlunparse_puts_the_params_back_after_a_semicolon() -> Result<(), String> {
    let parsed = urlparse("http://a/p;x?q#f", "")?;
    assert_eq!(urlunparse(&parsed, "f"), "http://a/p;x?q#f");
    Ok(())
}

#[test]
fn urlsplit_failures_surface_the_value_error_text() {
    assert_eq!(urljoin("http://[::1/a", "b"), Err("Invalid IPv6 URL".to_owned()));
    assert_eq!(urldefrag("http://[::1/p#f"), Err("Invalid IPv6 URL".to_owned()));
    assert_eq!(urldefrag("http://h/p#f"), Ok(("http://h/p".to_owned(), "f".to_owned())));
    assert_eq!(urldefrag("a"), Ok(("a".to_owned(), String::new())));
}

#[test]
fn bracketed_hosts_are_checked_like_ipaddress() {
    let cases: [(&str, Result<(), &str>); 8] = [
        ("x[@]y", Err("']y' does not appear to be an IPv4 or IPv6 address")),
        ("]@[", Err("'' does not appear to be an IPv4 or IPv6 address")),
        ("[::1]", Ok(())),
        ("[::1]:80", Ok(())),
        ("[v1.a]", Ok(())),
        ("[vg.a]", Err("IPvFuture address is invalid")),
        ("[v1.]", Err("IPvFuture address is invalid")),
        ("[1.2.3.4]", Err("An IPv4 address cannot be in brackets")),
    ];
    for (netloc, expected) in cases {
        assert_eq!(
            check_bracketed_netloc(netloc),
            expected.map_err(str::to_owned),
            "{netloc}"
        );
    }
    assert_eq!(check_bracketed_netloc("a[::1]"), Err("Invalid IPv6 URL".to_owned()));
    assert_eq!(check_bracketed_netloc("[::1]x"), Err("Invalid IPv6 URL".to_owned()));
    assert_eq!(
        check_bracketed_host("v1.a\nb"),
        Err("IPvFuture address is invalid".to_owned())
    );
    assert_eq!(
        check_bracketed_host("v"),
        Err("IPvFuture address is invalid".to_owned())
    );
}

#[test]
fn ipv4_validity_matches_ipaddress() {
    for valid in ["0.0.0.0", "1.2.3.4", "255.255.255.255", "10.0.0.1"] {
        assert!(is_ipv4(valid), "{valid}");
    }
    for invalid in [
        "",
        "1.2.3",
        "1.2.3.4.5",
        "1.2.3.",
        "1.2.3.04",
        "256.1.1.1",
        "1.2.3.1234",
        "1.2.3.a",
        "1.2.3.+1",
        "1.2.3.4/8",
        "1.2.3.\u{663}",
    ] {
        assert!(!is_ipv4(invalid), "{invalid}");
    }
}

#[test]
fn ipv6_validity_matches_ipaddress() {
    let valid = [
        "::",
        "::1",
        "1::",
        "1:2:3:4:5:6:7:8",
        "1:2:3:4:5:6:7::",
        "::2:3:4:5:6:7:8",
        "1::8",
        "::ffff:1.2.3.4",
        "fe80::1%eth0",
        "2001:DB8::1",
        "1:2:3:4:5:6:1.2.3.4",
    ];
    for text in valid {
        assert!(is_ipv6(text), "{text}");
    }
    let long = format!("1:{}1", "1:".repeat(30));
    let invalid = [
        "",
        "::1/64",
        "::1%",
        "::1%a%b",
        "1:2",
        "1",
        "::ffff:1.2.3",
        "1:2:3:4:5:6:7:8:9",
        "1:2:3:4:5:6:7:8:9:a:b",
        "1::2::3",
        ":1:2:3:4:5:6:7",
        "1:2:3:4:5:6:7:",
        "1:2:3:4::5:6:7:8",
        "1:2:3:4:5:6:7",
        ":1::2",
        "1::2:",
        "1:2:3:4:5:6:7:8:",
        "12345::1",
        "g::1",
        "1:2:3:4:5:6:7:8:1.2.3.4",
        long.as_str(),
        "1:::2",
    ];
    for text in invalid {
        assert!(!is_ipv6(text), "{text}");
    }
}

#[test]
fn netlocs_that_grow_delimiters_under_nfkc_are_rejected() {
    assert_eq!(check_netloc(""), Ok(()));
    assert_eq!(check_netloc("example.com"), Ok(()));
    assert_eq!(check_netloc("h\u{e9}.com"), Ok(()));
    assert_eq!(check_netloc("\u{ff21}.com"), Ok(()));
    assert_eq!(
        check_netloc("a\u{ff0f}b"),
        Err("netloc 'a\u{ff0f}b' contains invalid characters under NFKC normalization".to_owned())
    );
}

#[test]
fn urlsplit_strips_leading_controls_and_inner_line_breaks() {
    assert_eq!(
        urlsplit("\u{1} \thtt\np://a/b\r?c#d", ""),
        Ok([
            "http".to_owned(),
            "a".to_owned(),
            "/b".to_owned(),
            "c".to_owned(),
            "d".to_owned()
        ])
    );
    assert_eq!(
        urlsplit("1http://a", "x"),
        Ok([
            "x".to_owned(),
            String::new(),
            "1http://a".to_owned(),
            String::new(),
            String::new()
        ])
    );
}

#[test]
fn params_split_after_the_last_slash() {
    assert_eq!(split_params("a/b;c"), ("a/b", "c"));
    assert_eq!(split_params("a;b/c"), ("a;b/c", ""));
    assert_eq!(split_params("a;b"), ("a", "b"));
}

#[test]
fn unsplit_adds_the_delimiters_python_adds() {
    assert_eq!(urlunsplit("", "h", "p", "", ""), "//h/p");
    assert_eq!(urlunsplit("", "", "//p", "", ""), "////p");
    assert_eq!(urlunsplit("http", "", "", "", ""), "http://");
    assert_eq!(urlunsplit("urn", "", "a:b", "q", "f"), "urn:a:b?q#f");
}

#[test]
fn unquote_decodes_only_ascii_runs() {
    assert_eq!(unquote("%C3%A9%e9\u{e9}%4"), "\u{e9}\u{fffd}\u{e9}%4");
    assert_eq!(unquote("plain\u{e9}"), "plain\u{e9}");
    assert_eq!(unquote("%+1"), "%+1");
    assert_eq!(unquote("%4"), "%4");
    assert_eq!(unquote(""), "");
}

#[test]
fn geturl_normalizes_like_split_result_geturl() {
    assert_eq!(
        geturl("HTTP://json-schema.org/draft-07/schema#").as_deref(),
        Ok("http://json-schema.org/draft-07/schema")
    );
    assert_eq!(
        geturl("  https://json-schema.org/draft/2020-12/schema?").as_deref(),
        Ok("https://json-schema.org/draft/2020-12/schema")
    );
    assert_eq!(geturl("urn:x#").as_deref(), Ok("urn:x"));
    assert_eq!(geturl("//a/b").as_deref(), Ok("//a/b"));
    assert_eq!(geturl("http:").as_deref(), Ok("http://"));
    assert_eq!(geturl("http://[::1"), Err("Invalid IPv6 URL".to_owned()));
}

const SURROGATES: &str = include_str!("../tests/fixtures/sdk_mcp/python_urllib_surrogates.json");

#[test]
fn lone_surrogates_pass_through_as_cpython_passes_them() -> TestResult {
    let fixture = python_json_loads(SURROGATES)?;
    let cases = fixture
        .get("cases")
        .and_then(PythonJsonValue::as_array)
        .ok_or("fixture has no cases")?;
    assert_eq!(cases.len(), 60);
    for case in cases {
        let name = case
            .get("fn")
            .and_then(PythonJsonValue::as_str)
            .ok_or("case without fn")?;
        let inputs: Vec<&PythonString> = case
            .get("args")
            .and_then(PythonJsonValue::as_array)
            .ok_or("case without args")?
            .iter()
            .filter_map(PythonJsonValue::as_python_str)
            .collect();
        let text = |text: PythonString| PythonJsonValue::String(text);
        let actual = match (name, inputs.as_slice()) {
            ("urljoin", [base, url]) => urljoin(*base, *url).map(text),
            ("urldefrag", [url]) => {
                urldefrag(*url).map(|(without, fragment)| PythonJsonValue::Array(vec![text(without), text(fragment)]))
            }
            ("unquote", [input]) => Ok(text(unquote(*input))),
            ("geturl", [url]) => geturl(*url).map(text),
            _ => return Err(format!("unknown case {case:?}").into()),
        };
        let expected = match (case.get("ok"), case.get("error")) {
            (Some(ok), None) => Ok(ok.clone()),
            (None, Some(error)) => Err(error.as_python_str().ok_or("error is not a string")?.clone()),
            _ => return Err(format!("case without one outcome {case:?}").into()),
        };
        assert_eq!(actual, expected, "{case:?}");
    }
    Ok(())
}

/// `str.lower()`, `str.upper()` and the `Final_Sigma` context classes recorded from CPython 3.12 for every code point.
const CASE: &str = include_str!("../tests/fixtures/sdk_mcp/python_case.json");

fn mappings(fixture: &Value, name: &str) -> Result<std::collections::HashMap<u32, String>, Box<dyn std::error::Error>> {
    let mut mappings = std::collections::HashMap::new();
    for entry in fixture[name].as_array().ok_or("no mappings")? {
        let code = entry[0]
            .as_u64()
            .and_then(|code| u32::try_from(code).ok())
            .ok_or("no code point")?;
        mappings.insert(code, entry[1].as_str().ok_or("no mapping")?.to_owned());
    }
    Ok(mappings)
}

fn code_ranges(fixture: &Value, name: &str) -> Result<Vec<RangeInclusive<u32>>, Box<dyn std::error::Error>> {
    let mut ranges = Vec::new();
    for entry in fixture[name].as_array().ok_or("no ranges")? {
        let bound = |index: usize| entry[index].as_u64().and_then(|code| u32::try_from(code).ok());
        ranges.push(bound(0).ok_or("no start")?..=bound(1).ok_or("no end")?);
    }
    Ok(ranges)
}

#[test]
fn lower_and_upper_match_cpython_on_every_code_point() -> TestResult {
    let fixture: Value = serde_json::from_str(CASE)?;
    assert_eq!(fixture["unidata_version"], "15.0.0");
    let (lower, upper) = (mappings(&fixture, "lower")?, mappings(&fixture, "upper")?);
    assert_eq!((lower.len(), upper.len()), (1433, 1525));
    for c in (0..=0x10_ffff).filter_map(char::from_u32) {
        let (code, text) = (u32::from(c), c.to_string());
        let expected = |mappings: &std::collections::HashMap<u32, String>| {
            mappings.get(&code).cloned().unwrap_or_else(|| text.clone())
        };
        assert_eq!(python_lower(&text), expected(&lower), "lower {code:04X}");
        assert_eq!(python_upper(&text), expected(&upper), "upper {code:04X}");
    }
    Ok(())
}

#[test]
fn the_final_sigma_context_follows_unicode_15_on_every_code_point() -> TestResult {
    let fixture: Value = serde_json::from_str(CASE)?;
    let (cased, ignorable) = (
        code_ranges(&fixture, "cased")?,
        code_ranges(&fixture, "case_ignorable")?,
    );
    assert_eq!((cased.len(), ignorable.len()), (150, 437));
    for c in (0..=0x10_ffff).filter_map(char::from_u32) {
        let code = u32::from(c);
        let expected = if cased.iter().any(|range| range.contains(&code)) {
            SigmaContext::Cased
        } else if ignorable.iter().any(|range| range.contains(&code)) {
            SigmaContext::Ignorable
        } else {
            SigmaContext::Other
        };
        assert_eq!(sigma_context(c), expected, "{code:04X}");
    }
    Ok(())
}

#[test]
fn lower_and_upper_match_cpython_on_strings() -> TestResult {
    let fixture: Value = serde_json::from_str(CASE)?;
    let cases = fixture["strings"].as_array().ok_or("no strings")?;
    assert_eq!(cases.len(), 40);
    for case in cases {
        let text = case[0].as_str().ok_or("no text")?;
        assert_eq!(Some(python_lower(text).as_str()), case[1].as_str(), "lower {text:?}");
        assert_eq!(Some(python_upper(text).as_str()), case[2].as_str(), "upper {text:?}");
    }
    Ok(())
}
