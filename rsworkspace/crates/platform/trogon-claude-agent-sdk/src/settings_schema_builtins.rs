//! The upstream schema functions the interpreter reproduces, matched by their source text.
//!
//! The extracted description carries each function as source text plus the results it produced on a
//! fixed battery of inputs. Functions are recognised by source; a function that closes over a value
//! the source does not show (a platform name) is told apart by those recorded results.

use std::sync::OnceLock;

use serde_json::{Map, Value};

use crate::constants::{SETTINGS_SCHEMA_ARRAY_SECTIONS, SETTINGS_SCHEMA_REMOTE_SCHEMES};

static SENTINEL: OnceLock<Value> = OnceLock::new();

/// The marker upstream uses for a stripped invalid array entry. Upstream compares it by identity,
/// so it must never equal a value read from a document.
pub(crate) fn sentinel() -> &'static Value {
    SENTINEL.get_or_init(|| Value::String(format!("invalid-entry-stripped:{}", uuid::Uuid::new_v4())))
}

/// A refinement over the already type-checked value (a zod `custom` check).
#[derive(Debug)]
pub(crate) enum Predicate {
    NoNul,
    MaxUtf16(usize),
    NonBlank,
    Trimmed,
    NoSpace,
    NoParentOrDoubleSlash,
    NoSeparatorsOrParent,
    ExactlyOneServerField,
    NoInnerWildcard,
    RemoteOrNoParent,
    SourceNotString,
    SourceSupported,
    AsciiOnly,
    Always,
    Unported(String),
}

#[derive(Debug)]
pub(crate) enum Transform {
    NullToUndefined,
    Identity,
    MinOf(f64),
    Constant(&'static str),
    ManualToDefault,
    DotToDotSlash,
    BooleanToDetails,
    DropSentinel,
    KnownSections,
    StringsOrObjects,
    Unported(String),
}

pub(crate) fn transform_for(source: &str) -> Transform {
    match source {
        "(t)=>t===null?void 0:t" | "(n)=>n===null?void 0:n" => Transform::NullToUndefined,
        "(o)=>o" => Transform::Identity,
        "(e)=>Math.min(e,L5e)" => Transform::MinOf(5.0),
        "(t)=>Math.min(t,D5e)" | "(e)=>Math.min(e,D5e)" => Transform::MinOf(2_147_483_647.0),
        "()=>\"http\"" => Transform::Constant("http"),
        "function Cu(e){return e===\"manual\"?\"default\":e}" => Transform::ManualToDefault,
        "(e)=>e===\".\"?\"./\":e" => Transform::DotToDotSlash,
        "(s)=>{if(typeof s!==\"boolean\")return s;return s?{}:{commit:\"\",pr:\"\",sessionUrl:!1}}" => {
            Transform::BooleanToDetails
        }
        "(o)=>o.filter((s)=>s!==H3)" => Transform::DropSentinel,
        "(t)=>{if(!Array.isArray(t))return t;let n=t.filter((r)=>Qf.includes(r));if(n.length<t.length)e?.(t.length-n.length);return n}" => {
            Transform::KnownSections
        }
        "(e)=>Array.isArray(e)?e.filter((t)=>typeof t===\"string\"||!!t&&typeof t===\"object\"&&!Array.isArray(t)):[]" => {
            Transform::StringsOrObjects
        }
        other => Transform::Unported(other.to_owned()),
    }
}

impl Transform {
    pub(crate) fn unported_source(&self) -> Option<String> {
        match self {
            Transform::Unported(source) => Some(source.clone()),
            _ => None,
        }
    }

    pub(crate) fn apply(&self, input: Option<&Value>) -> Option<Value> {
        match self {
            Transform::NullToUndefined => match input {
                Some(Value::Null) => None,
                other => other.cloned(),
            },
            Transform::Identity | Transform::Unported(_) => input.cloned(),
            Transform::MinOf(limit) => match input {
                Some(Value::Number(number)) if number.as_f64().is_some_and(|value| value > *limit) => {
                    Some(Value::from(*limit as i64))
                }
                other => other.cloned(),
            },
            Transform::Constant(text) => Some(Value::String((*text).to_owned())),
            Transform::ManualToDefault => match input {
                Some(Value::String(text)) if text == "manual" => Some(Value::String("default".to_owned())),
                other => other.cloned(),
            },
            Transform::DotToDotSlash => match input {
                Some(Value::String(text)) if text == "." => Some(Value::String("./".to_owned())),
                other => other.cloned(),
            },
            Transform::BooleanToDetails => match input {
                Some(Value::Bool(true)) => Some(Value::Object(Map::new())),
                Some(Value::Bool(false)) => {
                    let mut details = Map::new();
                    details.insert("commit".to_owned(), Value::String(String::new()));
                    details.insert("pr".to_owned(), Value::String(String::new()));
                    details.insert("sessionUrl".to_owned(), Value::Bool(false));
                    Some(Value::Object(details))
                }
                other => other.cloned(),
            },
            Transform::DropSentinel => match input {
                Some(Value::Array(items)) => Some(Value::Array(
                    items.iter().filter(|item| *item != sentinel()).cloned().collect(),
                )),
                other => other.cloned(),
            },
            Transform::KnownSections => match input {
                Some(Value::Array(items)) => Some(Value::Array(
                    items
                        .iter()
                        .filter(|item| {
                            item.as_str()
                                .is_some_and(|name| SETTINGS_SCHEMA_ARRAY_SECTIONS.contains(&name))
                        })
                        .cloned()
                        .collect(),
                )),
                other => other.cloned(),
            },
            Transform::StringsOrObjects => Some(Value::Array(match input {
                Some(Value::Array(items)) => items
                    .iter()
                    .filter(|item| item.is_string() || item.is_object())
                    .cloned()
                    .collect(),
                _ => Vec::new(),
            })),
        }
    }
}

/// JavaScript `String.prototype.trim` white space (not the Unicode `White_Space` property).
pub(crate) fn is_js_whitespace(character: char) -> bool {
    matches!(
        character,
        '\t' | '\n' | '\u{b}' | '\u{c}' | '\r' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}'
    )
}

pub(crate) fn js_trim(text: &str) -> &str {
    text.trim_matches(is_js_whitespace)
}

fn utf16_length(text: &str) -> usize {
    text.encode_utf16().count()
}

fn has_remote_scheme(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    // The scheme list is case-insensitive in the upstream expression.
    SETTINGS_SCHEMA_REMOTE_SCHEMES
        .iter()
        .any(|scheme| lower.starts_with(scheme))
        || lower.starts_with("gitlab:")
        || lower.starts_with("bitbucket:")
}

pub(crate) fn predicate_for(source: &str, entry: &Value, battery: &[Value]) -> Predicate {
    let simple = match source {
        "(t)=>!t.includes(\"\\x00\")" => Some(Predicate::NoNul),
        "(e)=>e.length<=h3e" => Some(Predicate::MaxUtf16(65_536)),
        "(e)=>e.length<=_3e" => Some(Predicate::MaxUtf16(4096)),
        "(e)=>e.trim().length>0" => Some(Predicate::NonBlank),
        "(e)=>e===e.trim()" => Some(Predicate::Trimmed),
        "(t)=>!t.includes(\" \")" | "(e)=>!e.includes(\" \")" => Some(Predicate::NoSpace),
        "(e)=>!e.includes(\"..\")&&!e.includes(\"//\")" => Some(Predicate::NoParentOrDoubleSlash),
        "(t)=>!t.includes(\"/\")&&!t.includes(\"\\\\\")&&!t.includes(\"..\")&&t!==\".\"" => {
            Some(Predicate::NoSeparatorsOrParent)
        }
        "(e)=>yl([e.serverName!==void 0,e.serverCommand!==void 0,e.serverUrl!==void 0],Boolean)===1" => {
            Some(Predicate::ExactlyOneServerField)
        }
        "(e)=>!(e.endsWith(\"*\")?e.slice(0,-1):e).includes(\"*\")" => Some(Predicate::NoInnerWildcard),
        "(e)=>/^(?:file|https?|git(?:\\+https?|\\+ssh)?|ssh|github|gitlab|bitbucket):/i.test(e)||!e.includes(\"..\")" => {
            Some(Predicate::RemoteOrNoParent)
        }
        "(e)=>typeof e.source!==\"string\"" => Some(Predicate::SourceNotString),
        "(e)=>typeof e.source===\"string\"||e.source.source!==\"unsupported\"" => Some(Predicate::SourceSupported),
        _ => None,
    };
    if let Some(predicate) = simple {
        return predicate;
    }
    if source == "(t)=>e!==\"windows\"||!/[\\u0080-\\uffff]/.test(t)" {
        let recorded = entry
            .get("fn")
            .and_then(|function| function.get("probes"))
            .and_then(Value::as_array);
        let rendered = |predicate: &Predicate| -> Vec<String> {
            battery
                .iter()
                .map(|input| if predicate.passes(input) { "true" } else { "false" }.to_owned())
                .collect()
        };
        let recorded: Option<Vec<String>> = recorded.map(|probes| {
            probes
                .iter()
                .filter_map(|probe| probe.as_str().map(str::to_owned))
                .collect()
        });
        for candidate in [Predicate::Always, Predicate::AsciiOnly] {
            if recorded.as_ref() == Some(&rendered(&candidate)) {
                return candidate;
            }
        }
    }
    Predicate::Unported(source.to_owned())
}

impl Predicate {
    pub(crate) fn unported_source(&self) -> Option<String> {
        match self {
            Predicate::Unported(source) => Some(source.clone()),
            _ => None,
        }
    }

    /// `true` when the value satisfies the refinement; values of the wrong kind pass because the
    /// type check that precedes the refinement already rejected them.
    pub(crate) fn passes(&self, value: &Value) -> bool {
        match self {
            Predicate::Unported(_) | Predicate::Always => true,
            Predicate::NoNul => value.as_str().is_none_or(|text| !text.contains('\0')),
            Predicate::MaxUtf16(maximum) => value.as_str().is_none_or(|text| utf16_length(text) <= *maximum),
            Predicate::NonBlank => value.as_str().is_none_or(|text| !js_trim(text).is_empty()),
            Predicate::Trimmed => value.as_str().is_none_or(|text| js_trim(text) == text),
            Predicate::NoSpace => value.as_str().is_none_or(|text| !text.contains(' ')),
            Predicate::NoParentOrDoubleSlash => value
                .as_str()
                .is_none_or(|text| !text.contains("..") && !text.contains("//")),
            Predicate::NoSeparatorsOrParent => value
                .as_str()
                .is_none_or(|text| !text.contains('/') && !text.contains('\\') && !text.contains("..") && text != "."),
            Predicate::ExactlyOneServerField => value.as_object().is_none_or(|object| {
                ["serverName", "serverCommand", "serverUrl"]
                    .iter()
                    .filter(|key| object.contains_key(**key))
                    .count()
                    == 1
            }),
            Predicate::NoInnerWildcard => value
                .as_str()
                .is_none_or(|text| !text.strip_suffix('*').unwrap_or(text).contains('*')),
            Predicate::RemoteOrNoParent => value
                .as_str()
                .is_none_or(|text| has_remote_scheme(text) || !text.contains("..")),
            Predicate::SourceNotString => value
                .as_object()
                .is_none_or(|object| !object.get("source").is_some_and(Value::is_string)),
            Predicate::SourceSupported => value.as_object().is_none_or(|object| match object.get("source") {
                Some(Value::String(_)) => true,
                Some(Value::Object(inner)) => inner.get("source").and_then(Value::as_str) != Some("unsupported"),
                _ => true,
            }),
            Predicate::AsciiOnly => value.as_str().is_none_or(str::is_ascii),
        }
    }
}
