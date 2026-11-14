//! A zod issue and the English message the bundled locale gives it.

use crate::{
    JsonObject,
    sdk_mcp_js::{js_string, stringify_primitive},
};
use serde_json::Value;

/// One zod issue before it is reported: its fields in zod's key order, whether the check that raised it
/// lets later checks of the same schema run (`continue`), and the path it was raised at.
pub(crate) struct Issue {
    fields: JsonObject,
    received: &'static str,
    message: Option<String>,
    pub(crate) keeps_going: bool,
    pub(crate) path: Vec<Value>,
}

impl Issue {
    pub(crate) fn new<const N: usize>(fields: [(&str, Value); N], keeps_going: bool) -> Self {
        Self {
            fields: fields.into_iter().map(|(key, value)| (key.to_owned(), value)).collect(),
            received: "undefined",
            message: None,
            keeps_going,
            path: Vec::new(),
        }
    }

    /// An `invalid_type` issue; the report names the type of the value that was received.
    pub(crate) fn invalid_type(expected: &str, received: &'static str) -> Self {
        Self {
            received,
            ..Self::new([("expected", expected.into()), ("code", "invalid_type".into())], false)
        }
    }

    /// The `invalid_type` issue of a number that is not an integer: it names the integer format.
    pub(crate) fn invalid_int(format: &str) -> Self {
        Self {
            received: "number",
            ..Self::new(
                [
                    ("expected", "int".into()),
                    ("format", format.into()),
                    ("code", "invalid_type".into()),
                ],
                false,
            )
        }
    }

    /// A `custom` issue with the message of a refinement.
    pub(crate) fn custom(message: impl Into<String>) -> Self {
        Self {
            message: Some(message.into()),
            ..Self::new([("code", "custom".into())], true)
        }
    }

    /// The `custom` issue a transform adds with `ctx.addIssue`: the message is one of its fields, so it
    /// comes before the path, and the issue stops the checks after it.
    pub(crate) fn added(message: impl Into<String>) -> Self {
        let message: String = message.into();
        Self::new([("code", "custom".into()), ("message", message.into())], false)
    }

    /// The issue of a discriminated union whose discriminator matches none of its options.
    pub(crate) fn no_matching_discriminator(discriminator: &str) -> Self {
        Self::new(
            [
                ("code", "invalid_union".into()),
                ("errors", Value::Array(Vec::new())),
                ("note", "No matching discriminator".into()),
                ("discriminator", discriminator.into()),
            ],
            false,
        )
    }

    pub(crate) fn at(mut self, segment: Value) -> Self {
        self.path.insert(0, segment);
        self
    }

    /// The issue as zod reports it: its fields, then `path`, then `message`.
    pub(crate) fn report(&self) -> JsonObject {
        let mut report = self.fields.clone();
        report.insert("path".into(), Value::Array(self.path.clone()));
        if !self.fields.contains_key("message") {
            let message = match &self.message {
                Some(message) => message.clone(),
                None => message(&self.fields, self.received),
            };
            report.insert("message".into(), message.into());
        }
        report
    }
}

/// Whether a check that raised one of `issues` stops the checks after it (`aborted`).
pub(crate) fn aborted(issues: &[Issue]) -> bool {
    issues.iter().any(|issue| !issue.keeps_going)
}

fn format_name(format: &str) -> &str {
    match format {
        "regex" | "template_literal" => "input",
        "email" => "email address",
        "url" => "URL",
        "emoji" => "emoji",
        "uuid" => "UUID",
        "uuidv4" => "UUIDv4",
        "uuidv6" => "UUIDv6",
        "nanoid" => "nanoid",
        "guid" => "GUID",
        "cuid" => "cuid",
        "cuid2" => "cuid2",
        "ulid" => "ULID",
        "xid" => "XID",
        "ksuid" => "KSUID",
        "datetime" => "ISO datetime",
        "date" => "ISO date",
        "time" => "ISO time",
        "duration" => "ISO duration",
        "ipv4" => "IPv4 address",
        "ipv6" => "IPv6 address",
        "mac" => "MAC address",
        "cidrv4" => "IPv4 range",
        "cidrv6" => "IPv6 range",
        "base64" => "base64-encoded string",
        "base64url" => "base64url-encoded string",
        "json_string" => "JSON string",
        "e164" => "E.164 number",
        "credit_card" => "credit card number",
        "iban" => "IBAN",
        "jwt" => "JWT",
        other => other,
    }
}

fn size_unit(origin: &str) -> Option<&'static str> {
    match origin {
        "string" => Some("characters"),
        "file" => Some("bytes"),
        "array" | "set" => Some("items"),
        "map" => Some("entries"),
        _ => None,
    }
}

/// The message of the bundled English locale (`Hke`) for an issue.
fn message(fields: &JsonObject, received: &str) -> String {
    let text = |key: &str| fields.get(key).and_then(Value::as_str).unwrap_or_default();
    let flag = |key: &str| fields.get(key).and_then(Value::as_bool).unwrap_or(false);
    let number = |key: &str| fields.get(key).map(js_string).unwrap_or_default();
    let values = |key: &str| fields.get(key).and_then(Value::as_array).cloned().unwrap_or_default();
    let joined = |items: &[Value], separator: &str| {
        items
            .iter()
            .map(stringify_primitive)
            .collect::<Vec<_>>()
            .join(separator)
    };
    match text("code") {
        "invalid_type" => format!("Invalid input: expected {}, received {received}", text("expected")),
        "invalid_value" => match values("values").as_slice() {
            [only] => format!("Invalid input: expected {}", stringify_primitive(only)),
            options => format!("Invalid option: expected one of {}", joined(options, "|")),
        },
        "too_big" => {
            let bound = if flag("exact") {
                "exactly "
            } else if flag("inclusive") {
                "<="
            } else {
                "<"
            };
            let origin = fields.get("origin").and_then(Value::as_str).unwrap_or("value");
            match size_unit(origin) {
                Some(unit) => format!("Too big: expected {origin} to have {bound}{} {unit}", number("maximum")),
                None => format!("Too big: expected {origin} to be {bound}{}", number("maximum")),
            }
        }
        "too_small" => {
            let bound = if flag("exact") {
                "exactly "
            } else if flag("inclusive") {
                ">="
            } else {
                ">"
            };
            let origin = text("origin");
            match size_unit(origin) {
                Some(unit) => format!(
                    "Too small: expected {origin} to have {bound}{} {unit}",
                    number("minimum")
                ),
                None => format!("Too small: expected {origin} to be {bound}{}", number("minimum")),
            }
        }
        "invalid_format" => match text("format") {
            "starts_with" => format!("Invalid string: must start with \"{}\"", text("prefix")),
            "ends_with" => format!("Invalid string: must end with \"{}\"", text("suffix")),
            "includes" => format!("Invalid string: must include \"{}\"", text("includes")),
            "regex" => format!("Invalid string: must match pattern {}", text("pattern")),
            format => format!("Invalid {}", format_name(format)),
        },
        "not_multiple_of" => format!("Invalid number: must be a multiple of {}", number("divisor")),
        "unrecognized_keys" => {
            let keys = values("keys");
            let plural = if keys.len() > 1 { "s" } else { "" };
            format!("Unrecognized key{plural}: {}", joined(&keys, ", "))
        }
        "invalid_key" => format!("Invalid key in {}", text("origin")),
        _ => "Invalid input".to_owned(),
    }
}
