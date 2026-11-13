//! Interpreter for the settings schema extracted from the pinned TypeScript bundle.
//!
//! The extracted description (`upstream/settings_schema.json`) is a zod v4 schema tree. This module
//! rebuilds it and validates a settings document with the same semantics as `safeParse`: it yields
//! the parsed output (transforms applied, defaults from `catch` filled in) or a failure.

use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, OnceLock},
};

use serde_json::{Map, Value};
use url::Url;

use crate::{
    constants::{SETTINGS_SCHEMA_JSON, SETTINGS_SCHEMA_SAFE_INTEGER},
    settings_merge_spec::MergeSpec,
    settings_numbers::js_value,
    settings_read_spec::ReadSpec,
    settings_schema_builtins::{
        Batteries, Payload, Predicate, Transform, payload_for, predicate_for, sentinel, transform_for,
    },
    settings_schema_ports::js_string,
};

#[derive(Debug, thiserror::Error)]
pub(crate) enum SettingsSchemaError {
    #[error("the settings schema description is not valid JSON")]
    Json(#[from] serde_json::Error),
    #[error("the settings schema description is malformed: {0}")]
    Malformed(String),
}

/// Validation never fails with detail: the resolver only needs to know whether the parse passed.
type Validation = Result<Option<Value>, ()>;

pub(crate) struct SettingsSchema {
    keys: Vec<String>,
    fields: HashMap<String, Arc<Node>>,
    /// The schemas the in-place sanitizers validate single entries with, by name.
    roots: HashMap<String, Arc<Node>>,
    read: ReadSpec,
    merge: MergeSpec,
}

#[derive(Debug)]
enum Mode {
    Strip,
    Strict,
    Passthrough,
}

#[derive(Debug)]
enum Check {
    MinLength(usize),
    MaxLength(usize),
    LengthEquals(usize),
    SafeInteger,
    GreaterThan {
        value: f64,
        inclusive: bool,
    },
    LessThan {
        value: f64,
        inclusive: bool,
    },
    Pattern(regex::bytes::Regex),
    StartsWith(String),
    Url,
    Predicate(Predicate),
    /// A refinement that reports through `issues` (upstream `superRefine` and `check`).
    Payload(Payload),
}

#[derive(Debug)]
enum Node {
    Optional(Arc<Node>),
    Str(Vec<Check>),
    /// `z.coerce.string()`: the input goes through JavaScript `String()` before the checks.
    CoercedStr(Vec<Check>),
    Num(Vec<Check>),
    Boolean,
    Unknown,
    Undefined,
    Literal(Vec<Value>),
    Enum(Vec<String>),
    Array {
        element: Arc<Node>,
        checks: Vec<Check>,
    },
    Union(Vec<Arc<Node>>),
    Record {
        key: Arc<Node>,
        value: Arc<Node>,
        checks: Vec<Check>,
    },
    Pipe {
        input: Arc<Node>,
        output: Arc<Node>,
    },
    Lazy(Arc<Node>),
    Catch {
        inner: Arc<Node>,
        value: Option<Value>,
    },
    Transform(Transform),
    Object {
        mode: Mode,
        shape: Vec<(String, Arc<Node>)>,
        checks: Vec<Check>,
    },
}

static SCHEMA: OnceLock<Option<SettingsSchema>> = OnceLock::new();

pub(crate) fn schema() -> Option<&'static SettingsSchema> {
    SCHEMA
        .get_or_init(|| match SettingsSchema::parse(SETTINGS_SCHEMA_JSON) {
            Ok(schema) => {
                tracing::debug!(
                    unported = schema.unported_sources().len(),
                    "settings schema functions not yet reproduced"
                );
                Some(schema)
            }
            Err(error) => {
                tracing::error!(error = %error, "settings schema description failed to load");
                None
            }
        })
        .as_ref()
}

impl SettingsSchema {
    pub(crate) fn parse(text: &str) -> Result<Self, SettingsSchemaError> {
        let document: Value = serde_json::from_str(text)?;
        let battery = |name: &str| -> Result<Vec<Value>, SettingsSchemaError> {
            Ok(array_of(&document["batteries"], name)?
                .iter()
                .map(expand_battery)
                .collect())
        };
        let batteries = Batteries {
            string: battery("string")?,
            object: battery("object")?,
            value: battery("value")?,
            rule: battery("rule")?,
        };
        let builder = Builder::new(&document, &batteries)?;
        let keys: Vec<String> = array_of(&document, "keys")?
            .iter()
            .map(|key| key.as_str().map(str::to_owned).ok_or_else(|| malformed("key")))
            .collect::<Result<_, _>>()?;
        let mut fields = HashMap::new();
        for (key, node) in object_of(&document, "fields")? {
            fields.insert(key.clone(), builder.node(node)?);
        }
        let read = document.get("read").ok_or_else(|| malformed("read"))?;
        let mut roots = HashMap::new();
        for (name, node) in object_of(read, "roots")? {
            roots.insert(name.clone(), builder.node(node)?);
        }
        Ok(Self {
            keys,
            fields,
            roots,
            read: ReadSpec::from_json(read).map_err(SettingsSchemaError::Malformed)?,
            merge: MergeSpec::from_json(document.get("merge").ok_or_else(|| malformed("merge"))?)
                .map_err(SettingsSchemaError::Malformed)?,
        })
    }

    pub(crate) fn read_spec(&self) -> &ReadSpec {
        &self.read
    }

    pub(crate) fn merge_spec(&self) -> &MergeSpec {
        &self.merge
    }

    /// Whether the named sanitizer schema accepts the entry (`safeParse(...).success`).
    pub(crate) fn root_accepts(&self, name: &str, value: &Value) -> bool {
        self.root_output(name, Some(value)).is_some()
    }

    /// The parsed output of the named sanitizer schema, or `None` when it rejects the entry.
    pub(crate) fn root_parse(&self, name: &str, value: &Value) -> Option<Value> {
        self.root_output(name, Some(value))?
    }

    pub(crate) fn root_output(&self, name: &str, value: Option<&Value>) -> Option<Option<Value>> {
        self.roots.get(name)?.validate(value).ok()
    }

    /// Top-level keys in schema order.
    pub(crate) fn keys(&self) -> &[String] {
        &self.keys
    }

    /// `forDocument(document).safeParse(document)`: only keys present in the document are checked,
    /// unknown keys pass through. Returns the parsed document, or `None` when validation fails.
    pub(crate) fn parse_document(&self, document: &Map<String, Value>) -> Option<Map<String, Value>> {
        let mut parsed = Map::new();
        // The document schema holds the present schema keys in schema order, then passes unknown keys
        // through in document order.
        for key in &self.keys {
            let (Some(value), Some(node)) = (document.get(key), self.fields.get(key)) else {
                continue;
            };
            if let Some(output) = node.validate(Some(value)).ok()? {
                parsed.insert(key.clone(), output);
            }
        }
        for (key, value) in document {
            if !self.fields.contains_key(key) {
                parsed.insert(key.clone(), value.clone());
            }
        }
        Some(parsed)
    }

    /// Source texts of the schema functions the interpreter does not reproduce yet.
    pub(crate) fn unported_sources(&self) -> Vec<String> {
        let mut found = Vec::new();
        for node in self.fields.values().chain(self.roots.values()) {
            node.collect_unported(&mut found);
        }
        found.sort();
        found.dedup();
        found
    }
}

fn malformed(what: &str) -> SettingsSchemaError {
    SettingsSchemaError::Malformed(what.to_owned())
}

fn array_of<'a>(value: &'a Value, key: &str) -> Result<&'a Vec<Value>, SettingsSchemaError> {
    value.get(key).and_then(Value::as_array).ok_or_else(|| malformed(key))
}

fn object_of<'a>(value: &'a Value, key: &str) -> Result<&'a Map<String, Value>, SettingsSchemaError> {
    value.get(key).and_then(Value::as_object).ok_or_else(|| malformed(key))
}

/// Rebuilds a battery input; a `{repeat: [char, count]}` marker stands for a long string.
pub(crate) fn expand_battery(input: &Value) -> Value {
    let repeated = input.get("repeat").and_then(Value::as_array).and_then(|parts| {
        let unit = parts.first()?.as_str()?;
        let count = usize::try_from(parts.get(1)?.as_u64()?).ok()?;
        Some(Value::String(unit.repeat(count)))
    });
    repeated.unwrap_or_else(|| input.clone())
}

struct Builder<'a> {
    definitions: &'a Map<String, Value>,
    batteries: &'a Batteries,
    built: std::cell::RefCell<HashMap<String, Arc<Node>>>,
    building: std::cell::RefCell<HashSet<String>>,
}

impl<'a> Builder<'a> {
    fn new(document: &'a Value, batteries: &'a Batteries) -> Result<Self, SettingsSchemaError> {
        Ok(Self {
            definitions: object_of(document, "definitions")?,
            batteries,
            built: Default::default(),
            building: Default::default(),
        })
    }

    fn node(&self, description: &Value) -> Result<Arc<Node>, SettingsSchemaError> {
        if let Some(name) = description.get("ref").and_then(Value::as_str) {
            if let Some(done) = self.built.borrow().get(name) {
                return Ok(done.clone());
            }
            if !self.building.borrow_mut().insert(name.to_owned()) {
                return Err(malformed("recursive definition"));
            }
            let body = self
                .definitions
                .get(name)
                .ok_or_else(|| malformed("unknown reference"))?;
            let node = self.node(body)?;
            self.building.borrow_mut().remove(name);
            self.built.borrow_mut().insert(name.to_owned(), node.clone());
            return Ok(node);
        }
        let kind = description
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| malformed("type"))?;
        let inner = |key: &str| -> Result<Arc<Node>, SettingsSchemaError> {
            self.node(description.get(key).ok_or_else(|| malformed(key))?)
        };
        let checks = || self.checks(description);
        let node = match kind {
            "optional" => Node::Optional(inner("inner")?),
            "string" if description.get("coerce") == Some(&Value::Bool(true)) => Node::CoercedStr(checks()?),
            "string" => Node::Str(checks()?),
            "number" => Node::Num(checks()?),
            "boolean" => Node::Boolean,
            "unknown" => Node::Unknown,
            "undefined" => Node::Undefined,
            "literal" => Node::Literal(array_of(description, "values")?.clone()),
            "enum" => Node::Enum(
                array_of(description, "values")?
                    .iter()
                    .map(|value| value.as_str().map(str::to_owned).ok_or_else(|| malformed("enum value")))
                    .collect::<Result<_, _>>()?,
            ),
            "array" => Node::Array {
                element: inner("element")?,
                checks: checks()?,
            },
            "union" => Node::Union(
                array_of(description, "options")?
                    .iter()
                    .map(|option| self.node(option))
                    .collect::<Result<_, _>>()?,
            ),
            "record" => Node::Record {
                key: inner("key")?,
                value: inner("value")?,
                checks: checks()?,
            },
            "pipe" => Node::Pipe {
                input: inner("in")?,
                output: inner("out")?,
            },
            "lazy" => Node::Lazy(inner("inner")?),
            "catch" => {
                let value = description.get("value").ok_or_else(|| malformed("catch value"))?;
                let value = match value.get("value") {
                    Some(value) if value.get("type").and_then(Value::as_str) == Some("invalid-entry-stripped") => {
                        Some(sentinel().clone())
                    }
                    Some(value) => Some(value.clone()),
                    None => None,
                };
                Node::Catch {
                    inner: inner("inner")?,
                    value,
                }
            }
            "transform" => {
                let source = description
                    .get("source")
                    .and_then(Value::as_str)
                    .ok_or_else(|| malformed("source"))?;
                Node::Transform(transform_for(source, description, self.batteries))
            }
            "object" => {
                let mode = match description.get("mode").and_then(Value::as_str) {
                    Some("strip") => Mode::Strip,
                    Some("strict") => Mode::Strict,
                    Some("passthrough") => Mode::Passthrough,
                    _ => return Err(malformed("object mode")),
                };
                let mut shape = Vec::new();
                for (key, value) in object_of(description, "shape")? {
                    shape.push((key.clone(), self.node(value)?));
                }
                Node::Object {
                    mode,
                    shape,
                    checks: checks()?,
                }
            }
            other => return Err(SettingsSchemaError::Malformed(format!("unsupported kind {other}"))),
        };
        Ok(Arc::new(node))
    }

    fn checks(&self, description: &Value) -> Result<Vec<Check>, SettingsSchemaError> {
        let Some(entries) = description.get("checks").and_then(Value::as_array) else {
            return Ok(Vec::new());
        };
        entries.iter().map(|entry| check_from(entry, self.batteries)).collect()
    }
}

fn check_from(entry: &Value, batteries: &Batteries) -> Result<Check, SettingsSchemaError> {
    let kind = entry
        .get("check")
        .and_then(Value::as_str)
        .ok_or_else(|| malformed("check"))?;
    let count = |key: &str| {
        entry
            .get(key)
            .and_then(Value::as_u64)
            .and_then(|count| usize::try_from(count).ok())
            .ok_or_else(|| malformed(key))
    };
    let bound = |key: &str| entry.get(key).and_then(Value::as_f64).ok_or_else(|| malformed(key));
    let inclusive = entry.get("inclusive").and_then(Value::as_bool).unwrap_or(false);
    Ok(match kind {
        "min_length" => Check::MinLength(count("minimum")?),
        "max_length" => Check::MaxLength(count("maximum")?),
        "length_equals" => Check::LengthEquals(count("length")?),
        "number_format" => match entry.get("format").and_then(Value::as_str) {
            Some("safeint") => Check::SafeInteger,
            _ => return Err(malformed("number format")),
        },
        "greater_than" => Check::GreaterThan {
            value: bound("value")?,
            inclusive,
        },
        "less_than" => Check::LessThan {
            value: bound("value")?,
            inclusive,
        },
        "string_format" => match entry.get("format").and_then(Value::as_str) {
            Some("regex") => {
                let pattern = entry
                    .get("pattern")
                    .and_then(|pattern| pattern.get("regex"))
                    .and_then(Value::as_str)
                    .ok_or_else(|| malformed("pattern"))?;
                let regex = regex::bytes::RegexBuilder::new(pattern)
                    .unicode(false)
                    .build()
                    .map_err(|error| SettingsSchemaError::Malformed(error.to_string()))?;
                Check::Pattern(regex)
            }
            Some("starts_with") => Check::StartsWith(
                entry
                    .get("prefix")
                    .and_then(Value::as_str)
                    .ok_or_else(|| malformed("prefix"))?
                    .to_owned(),
            ),
            Some("url") => Check::Url,
            _ => return Err(malformed("string format")),
        },
        "custom" => match (entry.get("fn"), entry.get("payloadCheck")) {
            (Some(function), _) => {
                let source = function
                    .get("source")
                    .and_then(Value::as_str)
                    .ok_or_else(|| malformed("function source"))?;
                Check::Predicate(predicate_for(source, entry, batteries))
            }
            (None, Some(payload)) => {
                let source = payload
                    .get("source")
                    .and_then(Value::as_str)
                    .ok_or_else(|| malformed("payload source"))?;
                Check::Payload(payload_for(source, payload, batteries))
            }
            (None, None) => return Err(malformed("custom check")),
        },
        other => return Err(SettingsSchemaError::Malformed(format!("unsupported check {other}"))),
    })
}

fn utf16_length(text: &str) -> usize {
    text.encode_utf16().count()
}

fn json_equal(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Number(left), Value::Number(right)) => js_value(left) == js_value(right),
        _ => left == right,
    }
}

impl Check {
    fn passes(&self, value: &Value) -> bool {
        match self {
            Check::MinLength(minimum) => length_of(value).is_none_or(|length| length >= *minimum),
            Check::MaxLength(maximum) => length_of(value).is_none_or(|length| length <= *maximum),
            Check::LengthEquals(expected) => length_of(value).is_none_or(|length| length == *expected),
            Check::SafeInteger => number_of(value)
                .is_none_or(|number| number.fract() == 0.0 && number.abs() <= SETTINGS_SCHEMA_SAFE_INTEGER),
            Check::GreaterThan {
                value: bound,
                inclusive,
            } => number_of(value).is_none_or(|number| if *inclusive { number >= *bound } else { number > *bound }),
            Check::LessThan {
                value: bound,
                inclusive,
            } => number_of(value).is_none_or(|number| if *inclusive { number <= *bound } else { number < *bound }),
            Check::Pattern(regex) => value.as_str().is_none_or(|text| regex.is_match(text.as_bytes())),
            Check::StartsWith(prefix) => value.as_str().is_none_or(|text| text.starts_with(prefix.as_str())),
            Check::Url => value.as_str().is_none_or(|text| Url::parse(text).is_ok()),
            Check::Predicate(predicate) => predicate.passes(value),
            Check::Payload(payload) => payload.issues(value) == 0,
        }
    }

    fn unported(&self) -> Option<String> {
        match self {
            Check::Predicate(predicate) => predicate.unported_source(),
            Check::Payload(payload) => payload.unported_source(),
            _ => None,
        }
    }
}

fn length_of(value: &Value) -> Option<usize> {
    match value {
        Value::String(text) => Some(utf16_length(text)),
        Value::Array(items) => Some(items.len()),
        _ => None,
    }
}

fn all_pass(checks: &[Check], value: &Value) -> bool {
    checks.iter().all(|check| check.passes(value))
}

impl Node {
    /// Whether the schema accepts a missing input (`_zod.optin === "optional"`).
    fn optional_in(&self) -> bool {
        match self {
            Node::Optional(_) => true,
            Node::Catch { inner, .. } | Node::Lazy(inner) => inner.optional_in(),
            Node::Pipe { input, .. } => input.optional_in(),
            Node::Union(options) => options.iter().any(|option| option.optional_in()),
            _ => false,
        }
    }

    /// Whether the schema may produce a missing output (`_zod.optout === "optional"`).
    fn optional_out(&self) -> bool {
        match self {
            Node::Optional(_) => true,
            Node::Catch { inner, .. } | Node::Lazy(inner) => inner.optional_out(),
            Node::Pipe { output, .. } => output.optional_out(),
            Node::Union(options) => options.iter().any(|option| option.optional_out()),
            _ => false,
        }
    }

    fn validate(&self, input: Option<&Value>) -> Validation {
        match self {
            Node::Optional(inner) => match input {
                None if !inner.optional_in() => Ok(None),
                _ => inner.validate(input),
            },
            Node::Str(checks) => match input {
                Some(value @ Value::String(_)) if all_pass(checks, value) => Ok(Some(value.clone())),
                _ => Err(()),
            },
            Node::CoercedStr(checks) => {
                let value = Value::String(input.map_or_else(|| "undefined".to_owned(), js_string));
                if all_pass(checks, &value) {
                    Ok(Some(value))
                } else {
                    Err(())
                }
            }
            // zod v4 `z.number()` refuses the infinities.
            Node::Num(checks) => match input {
                Some(value @ Value::Number(number))
                    if js_value(number).is_some_and(f64::is_finite) && all_pass(checks, value) =>
                {
                    Ok(Some(value.clone()))
                }
                _ => Err(()),
            },
            Node::Boolean => match input {
                Some(value @ Value::Bool(_)) => Ok(Some(value.clone())),
                _ => Err(()),
            },
            Node::Unknown => Ok(input.cloned()),
            Node::Undefined => match input {
                None => Ok(None),
                Some(_) => Err(()),
            },
            Node::Literal(values) => match input {
                Some(value) if values.iter().any(|expected| json_equal(expected, value)) => Ok(Some(value.clone())),
                _ => Err(()),
            },
            Node::Enum(values) => match input {
                Some(Value::String(text)) if values.iter().any(|expected| expected == text) => {
                    Ok(Some(Value::String(text.clone())))
                }
                _ => Err(()),
            },
            Node::Array { element, checks } => {
                let Some(Value::Array(items)) = input else {
                    return Err(());
                };
                let mut output = Vec::with_capacity(items.len());
                for item in items {
                    output.push(element.validate(Some(item))?.unwrap_or(Value::Null));
                }
                let output = Value::Array(output);
                if all_pass(checks, &output) {
                    Ok(Some(output))
                } else {
                    Err(())
                }
            }
            Node::Union(options) => options.iter().find_map(|option| option.validate(input).ok()).ok_or(()),
            Node::Record { key, value, checks } => {
                let Some(Value::Object(entries)) = input else {
                    return Err(());
                };
                let mut output = Map::new();
                for (name, entry) in entries {
                    key.validate(Some(&Value::String(name.clone())))?;
                    if let Some(parsed) = value.validate(Some(entry))? {
                        output.insert(name.clone(), parsed);
                    }
                }
                let output = Value::Object(output);
                if all_pass(checks, &output) {
                    Ok(Some(output))
                } else {
                    Err(())
                }
            }
            Node::Pipe { input: first, output } => {
                let middle = first.validate(input)?;
                output.validate(middle.as_ref())
            }
            Node::Lazy(inner) => inner.validate(input),
            Node::Catch { inner, value } => match inner.validate(input) {
                Ok(output) => Ok(output),
                Err(()) => Ok(value.clone()),
            },
            Node::Transform(transform) => Ok(transform.apply(input)),
            Node::Object { mode, shape, checks } => {
                let Some(Value::Object(entries)) = input else {
                    return Err(());
                };
                let mut output = Map::new();
                for (key, node) in shape {
                    let present = entries.get(key);
                    match node.validate(present) {
                        Ok(Some(parsed)) => {
                            output.insert(key.clone(), parsed);
                        }
                        Ok(None) => {}
                        Err(()) if present.is_none() && node.optional_in() && node.optional_out() => {}
                        Err(()) => return Err(()),
                    }
                }
                let known = |name: &String| shape.iter().any(|(key, _)| key == name);
                match mode {
                    Mode::Strip => {}
                    Mode::Strict if entries.keys().any(|name| !known(name)) => return Err(()),
                    Mode::Strict => {}
                    Mode::Passthrough => {
                        for (name, entry) in entries {
                            if !known(name) {
                                output.insert(name.clone(), entry.clone());
                            }
                        }
                    }
                }
                let output = Value::Object(output);
                if all_pass(checks, &output) {
                    Ok(Some(output))
                } else {
                    Err(())
                }
            }
        }
    }

    fn collect_unported(&self, found: &mut Vec<String>) {
        match self {
            Node::Optional(inner) | Node::Lazy(inner) | Node::Catch { inner, .. } => inner.collect_unported(found),
            Node::Str(checks) | Node::CoercedStr(checks) | Node::Num(checks) => {
                found.extend(checks.iter().filter_map(Check::unported))
            }
            Node::Array { element, checks } => {
                element.collect_unported(found);
                found.extend(checks.iter().filter_map(Check::unported));
            }
            Node::Union(options) => options.iter().for_each(|option| option.collect_unported(found)),
            Node::Record { key, value, checks } => {
                key.collect_unported(found);
                value.collect_unported(found);
                found.extend(checks.iter().filter_map(Check::unported));
            }
            Node::Pipe { input, output } => {
                input.collect_unported(found);
                output.collect_unported(found);
            }
            Node::Transform(transform) => found.extend(transform.unported_source()),
            Node::Object { shape, checks, .. } => {
                shape.iter().for_each(|(_, node)| node.collect_unported(found));
                found.extend(checks.iter().filter_map(Check::unported));
            }
            Node::Boolean | Node::Unknown | Node::Undefined | Node::Literal(_) | Node::Enum(_) => {}
        }
    }
}

/// The JavaScript number a settings value holds, with the number markers read back.
fn number_of(value: &Value) -> Option<f64> {
    value.as_number().and_then(js_value)
}

#[cfg(test)]
#[path = "settings_schema_tests.rs"]
mod tests;
