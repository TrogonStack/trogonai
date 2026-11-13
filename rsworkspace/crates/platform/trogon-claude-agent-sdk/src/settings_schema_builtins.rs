//! The upstream schema functions the interpreter reproduces, matched by their source text.
//!
//! The extracted description carries each function as source text plus the results it produced on a
//! fixed battery of inputs. Functions are recognised by source; a function that closes over a value
//! the source does not show (a platform name) is told apart by those recorded results.

use std::sync::OnceLock;

use serde_json::{Map, Value};

use crate::{
    constants::{SETTINGS_SCHEMA_ARRAY_SECTIONS, SETTINGS_SCHEMA_REMOTE_SCHEMES},
    session_prompt::js_trim,
    settings_schema_hosts::{is_http_url, is_public_https_url},
    settings_schema_paths::PathProfile,
    settings_schema_permission_rule::is_valid_permission_rule,
    settings_schema_ports::{
        aws_pair_issues, aws_pairs_across_issues, clean_credential_entry, convert_request_timeout,
        credential_env_issues, credential_file_issues, default_payload_issues, drop_prototype_keys, entry_shape_issues,
        has_no_bidi_characters, has_no_invisible_path_characters, is_absolute_posix, is_header_helper_safe,
        is_not_impersonating, is_not_reserved_marketplace, is_package_name, is_provider_name, is_server_key,
        managed_server_issues, marketplace_key_issues, number_value, reserved_name_issues, to_number,
    },
};

/// The fixed inputs the extraction ran every function on, by battery name.
pub(crate) struct Batteries {
    pub(crate) string: Vec<Value>,
    pub(crate) object: Vec<Value>,
    pub(crate) value: Vec<Value>,
    pub(crate) rule: Vec<Value>,
}

impl Batteries {
    pub(crate) fn named(&self, name: &str) -> &[Value] {
        match name {
            "string" => &self.string,
            "object" => &self.object,
            "rule" => &self.rule,
            _ => &self.value,
        }
    }
}

/// Number-aware JSON equality: JavaScript has one number type.
fn same_json(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Number(left), Value::Number(right)) => left.as_f64() == right.as_f64(),
        (Value::Array(left), Value::Array(right)) => {
            left.len() == right.len() && left.iter().zip(right).all(|(a, b)| same_json(a, b))
        }
        (Value::Object(left), Value::Object(right)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .all(|(key, value)| right.get(key).is_some_and(|other| same_json(value, other)))
        }
        _ => left == right,
    }
}

/// Whether a recorded probe result equals what the port produced. A probe that threw upstream
/// (`error: ...`) is not comparable and is skipped.
pub(crate) fn probe_matches(recorded: &str, produced: Option<&Value>) -> bool {
    if recorded.starts_with("error:") {
        return true;
    }
    match (recorded, produced) {
        ("absent", None) => true,
        ("absent", Some(_)) | (_, None) => false,
        (text, Some(value)) => serde_json::from_str::<Value>(text).is_ok_and(|expected| same_json(&expected, value)),
    }
}

fn recorded_probes(owner: &Value) -> Vec<String> {
    owner
        .get("probes")
        .and_then(Value::as_array)
        .map(|probes| {
            probes
                .iter()
                .filter_map(|probe| probe.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

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
    InvisiblePathFree,
    PathAbsolute(PathProfile),
    PathSuffix(PathProfile),
    PathNoGlob(PathProfile),
    PathNormalized(PathProfile),
    HeaderSafe,
    HttpUrl,
    PackageName,
    NoBidi,
    NotImpersonating,
    NotReservedMarketplace,
    PublicHttps,
    ServerKey,
    Unported(String),
}

/// A check that reports through `issues` instead of returning a boolean.
#[derive(Debug)]
pub(crate) enum Payload {
    PolicyHelpers,
    ReservedName,
    EntryShape,
    ManagedServer,
    MarketplaceKey,
    CredentialFile,
    CredentialEnv,
    AwsPairDistinct,
    AwsPairsAcross,
    /// Upstream `Kk` and its helpers. The `"allow"` and default modes share one source text, so the
    /// recorded probes tell them apart.
    PermissionRule {
        allow: bool,
    },
    Unported(String),
}

pub(crate) fn payload_for(source: &str, owner: &Value, batteries: &Batteries) -> Payload {
    let starts = |prefix: &str| source.starts_with(prefix);
    if starts("(n)=>{if(n.issues.length>0)return;let r=G5e(n.value)") {
        Payload::EntryShape
    } else if starts("(e,t)=>{for(let n of[\"policyHelper\",\"policyHelpers\"])") {
        Payload::PolicyHelpers
    } else if starts("(t,n)=>{let r=Kk(t,e);") {
        permission_rule_mode(owner, batteries).map_or_else(
            || Payload::Unported(source.to_owned()),
            |allow| Payload::PermissionRule { allow },
        )
    } else if starts("(e)=>{let t=(r,o)=>{e.issues.push(") && source.contains("E3e") {
        Payload::ManagedServer
    } else if starts("(s)=>{for(let[i,a]of Object.entries(s.value))if(a.source.source===\"settings\"") {
        Payload::MarketplaceKey
    } else if starts("(t,n)=>{let r=t.toLowerCase();if(!Pk(r))return;") {
        Payload::ReservedName
    } else if starts("(e,t)=>{if(e.mode===\"mask\"&&e.path.endsWith(\"/\"))") {
        Payload::CredentialFile
    } else if starts("(e,t)=>{if(e.mode===\"mask\"&&e.extract!==void 0)D9(") {
        Payload::CredentialEnv
    } else if starts("(e,t)=>{let n=new Map;for(let[r,o]of[[\"accessKeyIdVar\"") {
        Payload::AwsPairDistinct
    } else if starts("(e,t)=>{let n=new Set;for(let[r,o]of(e?.awsPairs??[]).entries())") {
        Payload::AwsPairsAcross
    } else {
        Payload::Unported(source.to_owned())
    }
}

/// The mode whose verdicts reproduce every recorded probe of a permission rule check.
fn permission_rule_mode(owner: &Value, batteries: &Batteries) -> Option<bool> {
    let recorded = recorded_probes(owner);
    let battery = batteries.named("rule");
    if recorded.len() != battery.len() {
        return None;
    }
    [false, true].into_iter().find(|allow| {
        battery.iter().zip(&recorded).all(|(input, expected)| {
            serde_json::from_str::<Vec<Value>>(expected)
                .is_ok_and(|issues| issues.len() == permission_rule_issues(input, *allow))
        })
    })
}

fn permission_rule_issues(value: &Value, allow: bool) -> usize {
    value
        .as_str()
        .map_or(0, |rule| usize::from(!is_valid_permission_rule(rule, allow)))
}

impl Payload {
    pub(crate) fn unported_source(&self) -> Option<String> {
        match self {
            Payload::Unported(source) => Some(source.clone()),
            _ => None,
        }
    }

    /// How many issues the check raises on `value`.
    pub(crate) fn issues(&self, value: &Value) -> usize {
        match self {
            Payload::PolicyHelpers => default_payload_issues(value),
            Payload::ReservedName => reserved_name_issues(value),
            Payload::EntryShape => entry_shape_issues(value),
            Payload::ManagedServer => managed_server_issues(value),
            Payload::MarketplaceKey => marketplace_key_issues(value),
            Payload::CredentialFile => credential_file_issues(value),
            Payload::CredentialEnv => credential_env_issues(value),
            Payload::AwsPairDistinct => aws_pair_issues(value),
            Payload::AwsPairsAcross => aws_pairs_across_issues(value),
            Payload::PermissionRule { allow } => permission_rule_issues(value, *allow),
            Payload::Unported(_) => 0,
        }
    }
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
    ObjectOrAbsent,
    AbsolutePathOrAbsent,
    ProviderFilter,
    DropPrototypeKeys,
    CleanCredentialEntry,
    ConvertRequestTimeout,
    FailureOrRefuse,
    /// A policy helper slot: `null` is dropped, and outside the `default` slot so is an object whose
    /// helper fields are all null.
    PolicySlot(bool),
    Unported(String),
}

fn all_null_helper(value: &Value) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    let keys = ["path", "script", "defaultSettings"];
    keys.iter().any(|key| object.get(*key).is_some_and(Value::is_null))
        && keys.iter().all(|key| object.get(*key).is_none_or(Value::is_null))
}

pub(crate) fn transform_for(source: &str, description: &Value, batteries: &Batteries) -> Transform {
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
        "(e)=>V(e)?e:void 0" => Transform::ObjectOrAbsent,
        "(e)=>typeof e===\"string\"&&v9(e)?e:void 0" => Transform::AbsolutePathOrAbsent,
        "(o)=>Array.isArray(o)?o.filter(gk):o" => Transform::ProviderFilter,
        "(o)=>typeof o===\"object\"&&o!==null&&!Array.isArray(o)?Ja(o,(s,i)=>Object.hasOwn(Object.prototype,i)):o" => {
            Transform::DropPrototypeKeys
        }
        source if source.starts_with("function I9(e)") => Transform::CleanCredentialEntry,
        source if source.starts_with("function s6(") => Transform::ConvertRequestTimeout,
        "(e)=>e===void 0||X3(e)?e:\"refuse\"" => Transform::FailureOrRefuse,
        "(t)=>{if(t===null)return;if(e!==\"default\"&&Z3(t))return;return t}" => {
            let recorded = recorded_probes(description);
            let battery = batteries.named("value");
            [false, true]
                .into_iter()
                .map(Transform::PolicySlot)
                .find(|candidate| {
                    battery
                        .iter()
                        .zip(&recorded)
                        .all(|(input, expected)| probe_matches(expected, candidate.apply(Some(input)).as_ref()))
                })
                .unwrap_or_else(|| Transform::Unported(source.to_owned()))
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
            Transform::MinOf(limit) => {
                input.map(|value| to_number(value).map_or(Value::Null, |number| number_value(number.min(*limit))))
            }
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
            Transform::ObjectOrAbsent => input.filter(|value| value.is_object()).cloned(),
            Transform::AbsolutePathOrAbsent => input
                .filter(|value| value.as_str().is_some_and(is_absolute_posix))
                .cloned(),
            Transform::ProviderFilter => match input {
                Some(Value::Array(items)) => Some(Value::Array(
                    items.iter().filter(|item| is_provider_name(item)).cloned().collect(),
                )),
                other => other.cloned(),
            },
            Transform::DropPrototypeKeys => input.map(drop_prototype_keys),
            Transform::CleanCredentialEntry => input.map(clean_credential_entry),
            Transform::ConvertRequestTimeout => input.map(convert_request_timeout),
            Transform::FailureOrRefuse => match input {
                None => None,
                Some(Value::String(text)) if text == "continue" || text == "refuse" => input.cloned(),
                Some(_) => Some(Value::String("refuse".to_owned())),
            },
            Transform::PolicySlot(is_default) => match input {
                Some(Value::Null) => None,
                Some(value) if !*is_default && all_null_helper(value) => None,
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

/// Picks the candidate whose results match what the extraction recorded for a function that closes
/// over a value its source does not show.
fn select_predicate(owner: &Value, battery: &[Value], candidates: Vec<Predicate>) -> Option<Predicate> {
    let recorded = recorded_probes(owner);
    candidates.into_iter().find(|candidate| {
        battery.iter().zip(&recorded).all(|(input, expected)| {
            expected.starts_with("error:") || *expected == if candidate.passes(input) { "true" } else { "false" }
        })
    })
}

pub(crate) fn predicate_for(source: &str, entry: &Value, batteries: &Batteries) -> Predicate {
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
        "(r)=>!C5e.test(r)" => Some(Predicate::InvisiblePathFree),
        "(t)=>Ad(t)" => Some(Predicate::Always),
        "(e)=>!B6.test(e)" => Some(Predicate::HeaderSafe),
        "function $6(e){if(!/^https?:\\/\\/[^/\\\\]/i.test(e))return!1;try{let{protocol:t}=new URL(e);return t===\"https:\"||t===\"http:\"}catch{return!1}}" => {
            Some(Predicate::HttpUrl)
        }
        "(e)=>a3(e)" => Some(Predicate::PackageName),
        "(t)=>!R3.test(t)" | "(e)=>!R3.test(e)" => Some(Predicate::NoBidi),
        "(t)=>!kk.has(t.toLowerCase())" => Some(Predicate::NotReservedMarketplace),
        "function HJe(e){try{let t=new URL(e);return t.protocol===\"https:\"&&!g6(t.hostname)}catch{return!1}}" => {
            Some(Predicate::PublicHttps)
        }
        "function dk(e){return/^[A-Za-z0-9_-]+$/.test(e)&&!r6(e)&&e!==\"constructor\"&&e!==\"prototype\"}" => {
            Some(Predicate::ServerKey)
        }
        _ => None,
    };
    if let Some(predicate) = simple {
        return predicate;
    }
    let owner = entry.get("fn").unwrap_or(entry);
    let battery = batteries.named(owner.get("battery").and_then(Value::as_str).unwrap_or("string"));
    let candidates = match source {
        "(t)=>e!==\"windows\"||!/[\\u0080-\\uffff]/.test(t)" => vec![Predicate::Always, Predicate::AsciiOnly],
        "(r)=>(t===\"win32\"?V3:W3).isAbsolute(r)" => {
            vec![
                Predicate::PathAbsolute(PathProfile::Windows),
                Predicate::PathAbsolute(PathProfile::Posix),
            ]
        }
        "(r)=>!(t===\"win32\"&&n.requireWin32ExecutableSuffix)||w5e.test(r)" => {
            vec![
                Predicate::PathSuffix(PathProfile::Windows),
                Predicate::PathSuffix(PathProfile::Posix),
            ]
        }
        "(r)=>t!==\"win32\"||!x5e(r)" => {
            vec![
                Predicate::PathNoGlob(PathProfile::Windows),
                Predicate::PathNoGlob(PathProfile::Posix),
            ]
        }
        "(r)=>T5e(r,t,n)" => {
            vec![
                Predicate::PathNormalized(PathProfile::Windows),
                Predicate::PathNormalized(PathProfile::Posix),
            ]
        }
        "(t)=>!e(t)" => vec![Predicate::Always, Predicate::NotImpersonating],
        _ => Vec::new(),
    };
    select_predicate(owner, battery, candidates).unwrap_or_else(|| Predicate::Unported(source.to_owned()))
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
            Predicate::ExactlyOneServerField => value.as_object().is_some_and(|object| {
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
            Predicate::InvisiblePathFree => value.as_str().is_none_or(has_no_invisible_path_characters),
            Predicate::PathAbsolute(profile) => value.as_str().is_none_or(|text| profile.is_absolute(text)),
            Predicate::PathSuffix(profile) => value.as_str().is_none_or(|text| profile.has_executable_suffix(text)),
            Predicate::PathNoGlob(profile) => value.as_str().is_none_or(|text| profile.has_no_glob(text)),
            Predicate::PathNormalized(profile) => value.as_str().is_none_or(|text| profile.is_normalized(text)),
            Predicate::HeaderSafe => value.as_str().is_none_or(is_header_helper_safe),
            Predicate::HttpUrl => value.as_str().is_none_or(is_http_url),
            Predicate::PackageName => value.as_str().is_none_or(is_package_name),
            Predicate::NoBidi => value.as_str().is_none_or(has_no_bidi_characters),
            Predicate::NotImpersonating => value.as_str().is_none_or(is_not_impersonating),
            Predicate::NotReservedMarketplace => value.as_str().is_none_or(is_not_reserved_marketplace),
            Predicate::PublicHttps => value.as_str().is_none_or(is_public_https_url),
            Predicate::ServerKey => value.as_str().is_none_or(is_server_key),
        }
    }
}
