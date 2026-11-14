//! Ports of the remaining upstream schema functions: name and package validators, character class
//! tests, payload checks that report through `issues`, and the value cleaning transforms.

use std::sync::LazyLock;

use regex::{Regex, bytes::RegexBuilder};
use serde_json::{Map, Value};

use crate::constants::{
    SETTINGS_AWS_PAIR_SLOTS, SETTINGS_BUILTIN_PLUGIN_NAMES, SETTINGS_CREDENTIAL_DENY_MODE,
    SETTINGS_MANAGED_SERVER_FORBIDDEN, SETTINGS_MANAGED_SERVER_TYPES, SETTINGS_OBJECT_PROTOTYPE_NAMES,
    SETTINGS_PATTERN_BIDI_NAME, SETTINGS_PATTERN_ENV_REFERENCE, SETTINGS_PATTERN_FORMAT_CHARS,
    SETTINGS_PATTERN_IMPERSONATION, SETTINGS_PATTERN_INVISIBLE_PATH, SETTINGS_PATTERN_PLAIN_PACKAGE,
    SETTINGS_PATTERN_SCOPED_PACKAGE, SETTINGS_PAYLOAD_FORBIDDEN_KEYS, SETTINGS_PROVIDER_NAMES,
    SETTINGS_REQUEST_TIMEOUT_LIMIT, SETTINGS_RESERVED_MARKETPLACE_NAMES,
};
use crate::{
    settings_json::{js_float, js_number},
    settings_numbers::{finite_number, infinity, js_value},
    settings_schema_hosts::is_https_with_host,
};

fn unicode_pattern(pattern: &str) -> Option<Regex> {
    Regex::new(pattern).ok()
}

fn ascii_pattern(pattern: &str, insensitive: bool) -> Option<regex::bytes::Regex> {
    RegexBuilder::new(pattern)
        .case_insensitive(insensitive)
        .unicode(false)
        .build()
        .ok()
}

static INVISIBLE_PATH: LazyLock<Option<Regex>> = LazyLock::new(|| unicode_pattern(SETTINGS_PATTERN_INVISIBLE_PATH));
static BIDI_NAME: LazyLock<Option<Regex>> = LazyLock::new(|| unicode_pattern(SETTINGS_PATTERN_BIDI_NAME));
static FORMAT_CHARS: LazyLock<Option<Regex>> = LazyLock::new(|| unicode_pattern(SETTINGS_PATTERN_FORMAT_CHARS));
static IMPERSONATION: LazyLock<Option<regex::bytes::Regex>> =
    LazyLock::new(|| ascii_pattern(SETTINGS_PATTERN_IMPERSONATION, true));
static ENV_REFERENCE: LazyLock<Option<regex::bytes::Regex>> =
    LazyLock::new(|| ascii_pattern(SETTINGS_PATTERN_ENV_REFERENCE, false));
static SCOPED_PACKAGE: LazyLock<Option<regex::bytes::Regex>> =
    LazyLock::new(|| ascii_pattern(SETTINGS_PATTERN_SCOPED_PACKAGE, false));
static PLAIN_PACKAGE: LazyLock<Option<regex::bytes::Regex>> =
    LazyLock::new(|| ascii_pattern(SETTINGS_PATTERN_PLAIN_PACKAGE, false));

fn found(cell: &LazyLock<Option<Regex>>, text: &str) -> bool {
    cell.as_ref().is_some_and(|regex| regex.is_match(text))
}

fn found_bytes(cell: &LazyLock<Option<regex::bytes::Regex>>, text: &str) -> bool {
    cell.as_ref().is_some_and(|regex| regex.is_match(text.as_bytes()))
}

/// Upstream `!C5e.test(r)`.
pub(crate) fn has_no_invisible_path_characters(text: &str) -> bool {
    !found(&INVISIBLE_PATH, text)
}

/// Upstream `!R3.test(r)`.
pub(crate) fn has_no_bidi_characters(text: &str) -> bool {
    !found(&BIDI_NAME, text)
}

/// Upstream `!B6.test(e)`: printable ASCII with no run of four spaces.
pub(crate) fn is_header_helper_safe(text: &str) -> bool {
    text.chars().all(|character| ('\u{20}'..='\u{7e}').contains(&character)) && !text.contains("    ")
}

/// Upstream `a3`: an npm package name without `..` or `//`.
pub(crate) fn is_package_name(text: &str) -> bool {
    !text.contains("..")
        && !text.contains("//")
        && (found_bytes(&SCOPED_PACKAGE, text) || found_bytes(&PLAIN_PACKAGE, text))
}

fn is_reserved_name(lowered: &str) -> bool {
    SETTINGS_RESERVED_MARKETPLACE_NAMES.contains(&lowered)
}

/// Upstream `!kk.has(t.toLowerCase())`.
pub(crate) fn is_not_reserved_marketplace(text: &str) -> bool {
    !is_reserved_name(&text.to_lowercase())
}

/// Upstream `!q6(t)`: the marketplace name does not impersonate an official one.
pub(crate) fn is_not_impersonating(text: &str) -> bool {
    let lowered = text.to_lowercase();
    if is_reserved_name(&lowered) {
        return true;
    }
    let non_printable = text
        .chars()
        .any(|character| !('\u{20}'..='\u{7e}').contains(&character));
    !(non_printable || found_bytes(&IMPERSONATION, text))
}

/// Upstream `jf`.
fn sanitize_server_name(name: &str) -> String {
    let mut sanitized: String = name
        .chars()
        .flat_map(|character| {
            let units = character.len_utf16();
            let keep = character.is_ascii_alphanumeric() || character == '_' || character == '-';
            std::iter::repeat_n(if keep { character } else { '_' }, if keep { 1 } else { units })
        })
        .collect();
    if name.starts_with("claude.ai ") {
        let mut collapsed = String::new();
        for character in sanitized.chars() {
            if !(character == '_' && collapsed.ends_with('_')) {
                collapsed.push(character);
            }
        }
        let trimmed = collapsed.strip_prefix('_').unwrap_or(&collapsed);
        sanitized = trimmed.strip_suffix('_').unwrap_or(trimmed).to_owned();
    }
    sanitized
}

/// Upstream `n6`: `Au(ck(name) + "t") === null`.
fn parses_as_no_server(name: &str) -> bool {
    let candidate = format!("mcp__{}__t", sanitize_server_name(name));
    let mut parts = candidate.split("__");
    let first = parts.next();
    let second = parts.next();
    first != Some("mcp") || second.is_none_or(str::is_empty)
}

/// Upstream `r6`.
fn is_prototype_collision(name: &str) -> bool {
    SETTINGS_OBJECT_PROTOTYPE_NAMES.contains(&name) && parses_as_no_server(name)
}

/// Upstream `dk`: a server name that is safe to use as an object key.
pub(crate) fn is_server_key(text: &str) -> bool {
    !text.is_empty()
        && text
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_' || character == '-')
        && !is_prototype_collision(text)
        && text != "constructor"
        && text != "prototype"
}

/// Upstream `v9` on a POSIX host.
pub(crate) fn is_absolute_posix(text: &str) -> bool {
    text.starts_with('/')
}

fn present(object: &Map<String, Value>, key: &str) -> bool {
    object.get(key).is_some_and(|value| !value.is_null())
}

/// Upstream `al()`'s refinement: the number of issues a static `defaultSettings` payload raises.
pub(crate) fn default_payload_issues(value: &Value) -> usize {
    value.as_object().map_or(0, |object| {
        SETTINGS_PAYLOAD_FORBIDDEN_KEYS
            .iter()
            .filter(|key| present(object, key))
            .count()
    })
}

/// The marketplace name refinement: reserved session scoped names raise one issue.
pub(crate) fn reserved_name_issues(value: &Value) -> usize {
    value.as_str().map_or(0, |text| {
        usize::from(SETTINGS_BUILTIN_PLUGIN_NAMES.contains(&text.to_lowercase().as_str()))
    })
}

fn defined(object: &Map<String, Value>, key: &str) -> bool {
    object.contains_key(key)
}

/// Upstream `G5e` as a check: one issue when an entry's fields are inconsistent.
pub(crate) fn entry_shape_issues(value: &Value) -> usize {
    let Some(object) = value.as_object() else {
        // Destructuring a primitive or an array yields undefined fields, which is the missing-field issue;
        // `null` throws upstream and never reaches the check.
        return usize::from(!value.is_null());
    };
    let (path, script, interpreter, defaults) = (
        defined(object, "path"),
        defined(object, "script"),
        defined(object, "interpreter"),
        defined(object, "defaultSettings"),
    );
    let invalid = (path && script) || script != interpreter || (!path && !script && !defaults);
    usize::from(invalid)
}

/// Upstream `c6`: strings, and object keys with their values, to depth four.
fn flatten(value: &Value, depth: usize, found: &mut Vec<(String, bool)>) {
    match value {
        Value::String(text) => found.push((text.clone(), false)),
        _ if depth > 4 => {}
        Value::Object(entries) => {
            for (key, entry) in entries {
                found.push((key.clone(), true));
                flatten(entry, depth + 1, found);
            }
        }
        Value::Array(items) => {
            for (index, entry) in items.iter().enumerate() {
                found.push((index.to_string(), true));
                flatten(entry, depth + 1, found);
            }
        }
        _ => {}
    }
}

/// Upstream managed settings server check: the number of issues a managed MCP server raises.
pub(crate) fn managed_server_issues(value: &Value) -> usize {
    // `Object.hasOwn(null, ...)` throws upstream, so `null` never reaches the check; other primitives and
    // arrays have no own `type` or `url`, which still raises the type issue.
    if value.is_null() {
        return 0;
    }
    let empty = Map::new();
    let object = value.as_object().unwrap_or(&empty);
    let mut issues = SETTINGS_MANAGED_SERVER_FORBIDDEN
        .iter()
        .filter(|key| object.contains_key(**key))
        .count();
    let server_type = object.get("type").and_then(Value::as_str);
    if !server_type.is_some_and(|name| SETTINGS_MANAGED_SERVER_TYPES.contains(&name)) {
        issues += 1;
    }
    if object
        .get("url")
        .and_then(Value::as_str)
        .is_some_and(|url| !is_https_with_host(url))
    {
        issues += 1;
    }
    let mut entries = Vec::new();
    flatten(value, 0, &mut entries);
    for (text, is_key) in entries {
        if found(&FORMAT_CHARS, &text) || (!is_key && found_bytes(&ENV_REFERENCE, &text)) {
            issues += 1;
        }
    }
    issues
}

/// Upstream marketplace record check: one issue per settings sourced marketplace whose
/// `source.name` differs from its key.
pub(crate) fn marketplace_key_issues(value: &Value) -> usize {
    value.as_object().map_or(0, |entries| {
        entries
            .iter()
            .filter(|(key, entry)| {
                let source = entry.get("source");
                source.and_then(|source| source.get("source")).and_then(Value::as_str) == Some("settings")
                    && source.and_then(|source| source.get("name")).and_then(Value::as_str) != Some(key.as_str())
            })
            .count()
    })
}

/// Upstream `I9`: drops malformed optional fields from a `deny` credential entry.
pub(crate) fn clean_credential_entry(value: &Value) -> Value {
    let Some(object) = value.as_object() else {
        return value.clone();
    };
    if object.get("mode").and_then(Value::as_str) != Some(SETTINGS_CREDENTIAL_DENY_MODE) {
        return value.clone();
    }
    let strings = |entry: &Value| entry.as_array().is_some_and(|items| items.iter().all(Value::is_string));
    let mut cleaned = object.clone();
    let drop_unless = |cleaned: &mut Map<String, Value>, key: &str, keep: &dyn Fn(&Value) -> bool| {
        if cleaned.get(key).is_some_and(|entry| !keep(entry)) {
            cleaned.remove(key);
        }
    };
    drop_unless(&mut cleaned, "extract", &|entry| entry.is_string());
    drop_unless(&mut cleaned, "onExtractNoMatch", &|entry| {
        matches!(entry.as_str(), Some("warn" | "deny" | "error"))
    });
    drop_unless(&mut cleaned, "decode", &|entry| entry.as_str() == Some("jwt"));
    drop_unless(&mut cleaned, "maskClaims", &strings);
    drop_unless(&mut cleaned, "maskDuplicates", &|entry| entry.is_boolean());
    drop_unless(&mut cleaned, "injectHosts", &strings);
    Value::Object(cleaned)
}

/// JavaScript `Number(value)` for the JSON values a request timeout can carry; `None` stands for `NaN`.
pub(crate) fn to_number(value: &Value) -> Option<f64> {
    match value {
        Value::Number(number) => js_value(number).filter(|value| !value.is_nan()),
        Value::Null => Some(0.0),
        Value::Bool(flag) => Some(f64::from(u8::from(*flag))),
        Value::String(text) => string_to_number(text),
        Value::Array(items) => match items.as_slice() {
            [] => Some(0.0),
            [single] if !single.is_object() => match single {
                Value::Null => Some(0.0),
                other => to_number(other),
            },
            _ => None,
        },
        Value::Object(_) => None,
    }
}

fn string_to_number(text: &str) -> Option<f64> {
    let trimmed = crate::session_prompt::js_trim(text);
    if trimmed.is_empty() {
        return Some(0.0);
    }
    for (prefix, radix) in [("0x", 16), ("0X", 16), ("0o", 8), ("0O", 8), ("0b", 2), ("0B", 2)] {
        if let Some(digits) = trimmed.strip_prefix(prefix) {
            return u64::from_str_radix(digits, radix).ok().map(|value| value as f64);
        }
    }
    match trimmed {
        "Infinity" | "+Infinity" => Some(f64::INFINITY),
        "-Infinity" => Some(f64::NEG_INFINITY),
        _ if trimmed
            .chars()
            .all(|c| c.is_ascii_digit() || matches!(c, '.' | 'e' | 'E' | '+' | '-')) =>
        {
            trimmed.parse::<f64>().ok()
        }
        _ => None,
    }
}

/// A JavaScript number as a settings value: integral values stay integers, the infinities stay infinite until
/// the resolver prints them, and `NaN` becomes `null`.
pub(crate) fn number_value(number: f64) -> Value {
    if number.is_infinite() {
        return Value::Number(infinity(number < 0.0));
    }
    if number.fract() == 0.0 && number.abs() < 1e15 {
        Value::from(number as i64)
    } else {
        finite_number(number)
    }
}

/// Upstream `s6`: moves `request_timeout_ms` into `timeout` (capped) when no `timeout` is set.
pub(crate) fn convert_request_timeout(value: &Value) -> Value {
    // Object rest destructuring: primitives have no own enumerable keys except a string's indices, and an
    // array contributes its indices; `null` throws upstream and never reaches the transform.
    let object = match value {
        Value::Object(object) => object.clone(),
        Value::Array(items) => items
            .iter()
            .enumerate()
            .map(|(index, item)| (index.to_string(), item.clone()))
            .collect(),
        Value::String(text) => text
            .encode_utf16()
            .enumerate()
            .map(|(index, unit)| (index.to_string(), Value::String(String::from_utf16_lossy(&[unit]))))
            .collect(),
        Value::Null => return Value::Null,
        Value::Bool(_) | Value::Number(_) => Map::new(),
    };
    let mut rest = object;
    let requested = rest.remove("request_timeout_ms");
    if let Some(requested) = requested
        && !rest.contains_key("timeout")
    {
        let capped = to_number(&requested).map(|number| number.min(SETTINGS_REQUEST_TIMEOUT_LIMIT));
        let timeout = capped.map_or(Value::Null, number_value);
        rest.insert("timeout".to_owned(), timeout);
    }
    Value::Object(rest)
}

/// Upstream `Ja(o, (s, i) => Object.hasOwn(Object.prototype, i))`: drops keys that shadow `Object.prototype`.
pub(crate) fn drop_prototype_keys(value: &Value) -> Value {
    match value {
        Value::Object(object) => Value::Object(
            object
                .iter()
                .filter(|(key, _)| !SETTINGS_OBJECT_PROTOTYPE_NAMES.contains(&key.as_str()))
                .map(|(key, entry)| (key.clone(), entry.clone()))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// Upstream `gk`.
pub(crate) fn is_provider_name(value: &Value) -> bool {
    value
        .as_str()
        .is_some_and(|name| SETTINGS_PROVIDER_NAMES.contains(&name))
}

/// Upstream `D9`: one issue when `extract` is not a regular expression with a capturing group.
fn extract_issues(extract: &Value) -> usize {
    let source = match extract {
        Value::String(text) => text.clone(),
        other => js_string(other),
    };
    usize::from(crate::settings_schema_regexp::capturing_groups(&source).is_none_or(|groups| groups < 1))
}

/// JavaScript `String(value)` for the JSON values a settings document can hold.
pub(crate) fn js_string(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Null => "null".to_owned(),
        Value::Bool(flag) => flag.to_string(),
        Value::Number(number) => match js_value(number) {
            Some(value) if value.is_infinite() => if value > 0.0 { "Infinity" } else { "-Infinity" }.to_owned(),
            Some(value) if value.is_nan() => "NaN".to_owned(),
            Some(value) if number.is_f64() => js_float(value),
            _ => js_number(number),
        },
        Value::Array(items) => items
            .iter()
            .map(|item| if item.is_null() { String::new() } else { js_string(item) })
            .collect::<Vec<_>>()
            .join(","),
        Value::Object(_) => "[object Object]".to_owned(),
    }
}

/// Upstream `P9`: an empty claim list or claim, and claims without `decode`, each raise one issue.
fn mask_claims_issues(claims: &Value, decode: Option<&Value>) -> usize {
    let empty = is_zero_length(claims) || claims.as_array().is_some_and(|items| items.iter().any(is_zero_length));
    usize::from(empty) + usize::from(decode.is_none())
}

/// JavaScript `value.length === 0`.
fn is_zero_length(value: &Value) -> bool {
    match value {
        Value::String(text) => text.is_empty(),
        Value::Array(items) => items.is_empty(),
        Value::Object(object) => object.get("length").and_then(Value::as_f64) == Some(0.0),
        _ => false,
    }
}

fn is_mask(object: &Map<String, Value>) -> bool {
    object.get("mode").and_then(Value::as_str) == Some("mask")
}

/// Upstream credential file refinement: a masked directory, a bad `extract` and bad `maskClaims`.
pub(crate) fn credential_file_issues(value: &Value) -> usize {
    let Some(object) = value.as_object().filter(|object| is_mask(object)) else {
        return 0;
    };
    let mut issues = usize::from(
        object
            .get("path")
            .and_then(Value::as_str)
            .is_some_and(|path| path.ends_with('/')),
    );
    if let Some(extract) = object.get("extract") {
        issues += extract_issues(extract);
    }
    if let Some(claims) = object.get("maskClaims") {
        issues += mask_claims_issues(claims, object.get("decode"));
    }
    issues
}

/// Upstream credential environment variable refinement.
pub(crate) fn credential_env_issues(value: &Value) -> usize {
    let Some(object) = value.as_object().filter(|object| is_mask(object)) else {
        return 0;
    };
    let mut issues = 0;
    if let Some(extract) = object.get("extract") {
        issues += extract_issues(extract);
    }
    if let Some(claims) = object.get("maskClaims") {
        issues += mask_claims_issues(claims, object.get("decode"));
    }
    if object.contains_key("decode") {
        issues += usize::from(object.contains_key("extract"));
        issues += usize::from(matches!(
            object.get("onExtractNoMatch").and_then(Value::as_str),
            Some("deny" | "error")
        ));
    }
    issues
}

/// JavaScript `SameValueZero` for map keys: primitives by value, objects and arrays never equal.
fn same_key(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Object(_) | Value::Array(_), _) | (_, Value::Object(_) | Value::Array(_)) => false,
        (Value::Number(left), Value::Number(right)) => js_value(left) == js_value(right),
        _ => left == right,
    }
}

/// Upstream AWS pair refinement: a variable named twice within one pair raises an issue per repeat.
pub(crate) fn aws_pair_issues(value: &Value) -> usize {
    let Some(object) = value.as_object() else {
        return 0;
    };
    let mut seen: Vec<&Value> = Vec::new();
    let mut issues = 0;
    for slot in SETTINGS_AWS_PAIR_SLOTS {
        let Some(name) = object.get(slot) else { continue };
        if seen.iter().any(|known| same_key(known, name)) {
            issues += 1;
        } else {
            seen.push(name);
        }
    }
    issues
}

/// Upstream credentials refinement: a variable that fills slots in more than one pair.
pub(crate) fn aws_pairs_across_issues(value: &Value) -> usize {
    let Some(pairs) = value.get("awsPairs").and_then(Value::as_array) else {
        return 0;
    };
    let mut earlier: Vec<&Value> = Vec::new();
    let mut issues = 0;
    for pair in pairs {
        let mut current: Vec<&Value> = Vec::new();
        for slot in SETTINGS_AWS_PAIR_SLOTS {
            let Some(name) = pair.get(slot) else { continue };
            if earlier.iter().any(|known| same_key(known, name)) {
                issues += 1;
            }
            if !current.iter().any(|known| same_key(known, name)) {
                current.push(name);
            }
        }
        earlier.extend(current);
    }
    issues
}

#[cfg(test)]
#[path = "settings_schema_ports_tests.rs"]
mod tests;
