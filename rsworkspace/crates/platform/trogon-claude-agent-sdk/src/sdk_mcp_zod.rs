//! The zod parse of the TypeScript `McpServer`, rebuilt from the JSON Schema a tool advertises.
//!
//! `createSdkMcpServer` hands the MCP server a zod object whose properties are the zod types of the tool
//! author. The CLI only ever sees the JSON Schema `tools/list` derives from them, so the zod schema is
//! rebuilt from that document by reversing zod's own `toJSONSchema` mapping, and the parse runs with the
//! checks, issue order, abort rules, union reports and object modes of the bundled zod 4.6.2 and of the
//! zod 4.1.12 property schemas. The mapping and the parse are checked against the real bundle by
//! `tests/fixtures/sdk_mcp/typescript_zod.json`.
//!
//! Where JSON Schema cannot tell two zod schemas apart, the result is the one zod's own mapping makes
//! most likely: a discriminated union is a plain union, `nullable` and a union with `null` are the same,
//! `length(n)` is `min(n).max(n)`, regular expression flags are lost, and transforms and refinements are
//! invisible. The tool author overrides each of these with the `x-trogon-*` keywords that
//! `ValidationChoices::prepare` writes into a copy of the schema, which `tools/list` never sees; the
//! choices are checked by `tests/fixtures/sdk_mcp/typescript_choices.json`. The Rust regular expression and
//! URL engines stand in for the JavaScript ones.

use crate::{
    JsonObject, constants,
    sdk_mcp_choices::{SchemaCallback, StringCheckKind},
    sdk_mcp_js::ordered_keys,
    sdk_mcp_regexp,
    sdk_mcp_zod_issue::Issue,
};
use serde_json::{Number, Value};
use std::sync::Arc;

/// The callbacks of a tool's validation choices, indexed by the `x-trogon-callback` keyword of the schema nodes
/// they belong to.
pub(crate) type Callbacks<'a> = &'a [Arc<SchemaCallback>];

pub(crate) use crate::sdk_mcp_zod_run::{Input, run};

/// A number from the schema: kept as written for messages, as a float for comparisons.
#[derive(Clone)]
pub(crate) struct Bound {
    pub(crate) number: Number,
    pub(crate) value: f64,
}

impl Bound {
    fn new(number: &Number) -> Option<Self> {
        Some(Self {
            number: number.clone(),
            value: number.as_f64()?,
        })
    }

    fn of(schema: &JsonObject, key: &str) -> Option<Self> {
        match schema.get(key) {
            Some(Value::Number(number)) => Self::new(number),
            _ => None,
        }
    }

    fn integer(value: i64) -> Self {
        Self {
            number: Number::from(value),
            value: value as f64,
        }
    }

    pub(crate) fn json(&self) -> Value {
        Value::Number(self.number.clone())
    }
}

pub(crate) enum Affix {
    StartsWith,
    EndsWith,
    Includes,
}

pub(crate) enum StringCheck {
    None,
    Url,
    Affix {
        kind: Affix,
        text: String,
    },
    Pattern {
        source: String,
        format: String,
        /// The flags of the `RegExp`, which the listed JSON Schema does not carry (a validation choice).
        flags: String,
        matcher: Option<regress::Regex>,
    },
}

pub(crate) struct StringNode {
    pub(crate) min: Option<Bound>,
    pub(crate) max: Option<Bound>,
    pub(crate) exact: bool,
    /// The declared order of the minimum, maximum and format checks.
    pub(crate) order: [StringCheckKind; 3],
    pub(crate) check: StringCheck,
    pub(crate) base64: bool,
}

pub(crate) struct NumberNode {
    pub(crate) integer: bool,
    pub(crate) format: &'static str,
    pub(crate) low: Bound,
    pub(crate) high: Bound,
    pub(crate) min: Option<Bound>,
    pub(crate) exclusive_min: Option<Bound>,
    pub(crate) max: Option<Bound>,
    pub(crate) exclusive_max: Option<Bound>,
    pub(crate) multiple_of: Option<Bound>,
}

pub(crate) struct ArrayNode {
    pub(crate) element: Node,
    pub(crate) min: Option<Bound>,
    pub(crate) max: Option<Bound>,
    pub(crate) exact: bool,
}

pub(crate) enum ObjectMode {
    Strip,
    Strict,
    Catchall(Node),
}

pub(crate) struct ObjectNode {
    pub(crate) properties: Vec<(String, Node, bool)>,
    pub(crate) mode: ObjectMode,
}

pub(crate) enum Node {
    Any,
    Never,
    Boolean,
    Null,
    Literal(Vec<Value>),
    Optional(Box<Node>),
    Default(Box<Node>, Value),
    Catch(Box<Node>, Value),
    Nullable(Box<Node>),
    Union(Vec<Node>),
    /// `z.discriminatedUnion`: the option whose literal matches the value under `key`.
    Discriminated {
        key: String,
        options: Vec<(Vec<Value>, Node)>,
    },
    /// A schema with the user code of a `.refine` or `.transform`.
    Callback(Box<Node>, Arc<SchemaCallback>),
    Text(StringNode),
    Numeric(NumberNode),
    Array(Box<ArrayNode>),
    Tuple {
        items: Vec<Node>,
        rest: Option<Box<Node>>,
    },
    Object(ObjectNode),
    Record {
        key: Box<Node>,
        value: Box<Node>,
    },
    RecordEnum(Vec<(String, Node)>),
}

/// The result of a parse: the issues in zod's order, and the parsed value (`None` is `undefined`).
pub(crate) struct Parsed {
    pub(crate) issues: Vec<JsonObject>,
    pub(crate) value: Option<Value>,
}

impl Node {
    /// The zod schema a JSON Schema document stands for.
    pub(crate) fn from_schema(schema: &Value, callbacks: Callbacks<'_>) -> Self {
        modeled(schema, callbacks)
    }

    /// `schema.safeParse(input)`.
    pub(crate) fn parse(&self, input: Input<'_>) -> Parsed {
        let outcome = run(self, input);
        Parsed {
            issues: outcome.issues.iter().map(Issue::report).collect(),
            value: outcome.value.into_json(),
        }
    }
}

/// The node of a schema together with the callback its choices attach to it.
fn modeled(schema: &Value, callbacks: Callbacks<'_>) -> Node {
    let node = inner(schema, callbacks);
    let callback = schema
        .as_object()
        .and_then(|object| object.get("x-trogon-callback"))
        .and_then(Value::as_u64)
        .and_then(|index| usize::try_from(index).ok())
        .and_then(|index| callbacks.get(index));
    match callback {
        Some(callback) => Node::Callback(Box::new(node), Arc::clone(callback)),
        None => node,
    }
}

fn build(schema: &Value, required: bool, callbacks: Callbacks<'_>) -> Node {
    let node = modeled(schema, callbacks);
    match schema.as_object().and_then(|object| object.get("default")) {
        Some(default) if required => Node::Catch(Box::new(node), default.clone()),
        Some(default) => Node::Default(Box::new(node), default.clone()),
        None if required => node,
        None => Node::Optional(Box::new(node)),
    }
}

fn inner(schema: &Value, callbacks: Callbacks<'_>) -> Node {
    let Some(object) = schema.as_object() else {
        return Node::Any;
    };
    if object
        .get("not")
        .and_then(Value::as_object)
        .is_some_and(JsonObject::is_empty)
    {
        return Node::Never;
    }
    if let Some(options) = object.get("anyOf").and_then(Value::as_array) {
        if let Some(key) = object.get("x-trogon-discriminator").and_then(Value::as_str)
            && let Some(node) = discriminated(key, options, callbacks)
        {
            return node;
        }
        if let [first, second] = options.as_slice()
            && second
                .as_object()
                .is_some_and(|only| only.len() == 1 && only.get("type").and_then(Value::as_str) == Some("null"))
        {
            return null_or(build(first, true, callbacks), object);
        }
        return Node::Union(options.iter().map(|option| build(option, true, callbacks)).collect());
    }
    if let Some(values) = object.get("enum").and_then(Value::as_array) {
        return Node::Literal(values.clone());
    }
    if let Some(constant) = object.get("const") {
        return Node::Literal(vec![constant.clone()]);
    }
    if let Some(types) = object.get("type").and_then(Value::as_array) {
        let member = |kind: &Value| {
            let mut copy = object.clone();
            copy.insert("type".into(), kind.clone());
            inner(&Value::Object(copy), callbacks)
        };
        if let [first, second] = types.as_slice()
            && second.as_str() == Some("null")
        {
            return null_or(member(first), object);
        }
        return Node::Union(types.iter().map(member).collect());
    }
    match object.get("type").and_then(Value::as_str) {
        Some("string") => string(object),
        Some("number") => number(object, false),
        Some("integer") => number(object, true),
        Some("boolean") => Node::Boolean,
        Some("null") => Node::Null,
        Some("array") => array(object, callbacks),
        Some("object") => self::object(object, callbacks),
        _ => Node::Any,
    }
}

/// `z.nullable(schema)`, or `z.union([schema, z.null()])` when the schema's choices say so.
fn null_or(node: Node, schema: &JsonObject) -> Node {
    if schema.contains_key("x-trogon-union-with-null") {
        Node::Union(vec![node, Node::Null])
    } else {
        Node::Nullable(Box::new(node))
    }
}

/// The literals a discriminator property of an option can take: a `const`, an `enum` or `null`.
fn literals(property: &Value) -> Option<Vec<Value>> {
    let property = property.as_object()?;
    if let Some(constant) = property.get("const") {
        return Some(vec![constant.clone()]);
    }
    if let Some(values) = property.get("enum").and_then(Value::as_array) {
        return Some(values.clone());
    }
    (property.get("type").and_then(Value::as_str) == Some("null")).then(|| vec![Value::Null])
}

/// `z.discriminatedUnion(key, options)`; `None` when an option has no literal under the key.
fn discriminated(key: &str, options: &[Value], callbacks: Callbacks<'_>) -> Option<Node> {
    let mut built = Vec::with_capacity(options.len());
    for option in options {
        let values = option
            .as_object()
            .and_then(|object| object.get("properties"))
            .and_then(Value::as_object)
            .and_then(|properties| properties.get(key))
            .and_then(literals)?;
        built.push((values, build(option, true, callbacks)));
    }
    Some(Node::Discriminated {
        key: key.to_owned(),
        options: built,
    })
}

/// `text.replace(/\\(.)/g, "$1")`: the text a derived pattern escapes.
fn unescape(text: &str) -> String {
    let mut out = String::new();
    let mut characters = text.chars().peekable();
    while let Some(character) = characters.next() {
        match characters.peek() {
            Some(next) if character == '\\' && !matches!(next, '\n' | '\r' | '\u{2028}' | '\u{2029}') => {
                out.push(*next);
                characters.next();
            }
            _ => out.push(character),
        }
    }
    out
}

fn has_line_terminator(text: &str) -> bool {
    text.contains(['\n', '\r', '\u{2028}', '\u{2029}'])
}

fn string(schema: &JsonObject) -> Node {
    let min = Bound::of(schema, "minLength");
    let max = Bound::of(schema, "maxLength");
    let exact = is_exact(min.as_ref(), max.as_ref(), schema);
    let pattern = schema.get("pattern").and_then(Value::as_str);
    let format = schema.get("format").and_then(Value::as_str);
    let mut check = StringCheck::None;
    if pattern.is_some() || format.is_some() {
        let characters = pattern.map_or(0, |pattern| pattern.chars().count());
        check = match (format, pattern) {
            (Some("uri"), _) => StringCheck::Url,
            (Some("starts_with"), Some(pattern))
                if characters >= 3
                    && pattern.starts_with('^')
                    && pattern.ends_with(".*")
                    && !has_line_terminator(pattern) =>
            {
                StringCheck::Affix {
                    kind: Affix::StartsWith,
                    text: unescape(&pattern[1..pattern.len() - 2]),
                }
            }
            (Some("ends_with"), Some(pattern))
                if characters >= 3
                    && pattern.starts_with(".*")
                    && pattern.ends_with('$')
                    && !has_line_terminator(pattern) =>
            {
                StringCheck::Affix {
                    kind: Affix::EndsWith,
                    text: unescape(&pattern[2..pattern.len() - 1]),
                }
            }
            (Some("includes"), pattern) => StringCheck::Affix {
                kind: Affix::Includes,
                text: unescape(pattern.unwrap_or_default()),
            },
            (format, Some(pattern)) => {
                let flags = regexp_flags(schema, format);
                StringCheck::Pattern {
                    matcher: sdk_mcp_regexp::compile(pattern, &flags).ok(),
                    flags,
                    source: pattern.to_owned(),
                    format: match format {
                        None if schema.contains_key("x-trogon-iso-time") => "time",
                        None => "regex",
                        Some("date-time") => "datetime",
                        Some(other) => other,
                    }
                    .to_owned(),
                }
            }
            _ => StringCheck::None,
        };
    }
    Node::Text(StringNode {
        min,
        max,
        exact,
        order: check_order(schema),
        check,
        base64: schema.get("x-refine").and_then(Value::as_str) == Some("atob"),
    })
}

/// `.length(n)` rather than `.min(n).max(n)`: equal bounds, unless the choices say they were declared apart.
fn is_exact(min: Option<&Bound>, max: Option<&Bound>, schema: &JsonObject) -> bool {
    !schema.contains_key("x-trogon-min-max") && matches!((min, max), (Some(low), Some(high)) if low.value == high.value)
}

/// The declared order of the checks of a string: minimum, maximum, then the format, unless the choices name it.
fn check_order(schema: &JsonObject) -> [StringCheckKind; 3] {
    const DEFAULT: [StringCheckKind; 3] = [
        StringCheckKind::Minimum,
        StringCheckKind::Maximum,
        StringCheckKind::Format,
    ];
    let named: Vec<StringCheckKind> = schema
        .get("x-trogon-check-order")
        .and_then(Value::as_array)
        .map(|names| {
            names
                .iter()
                .filter_map(|name| match name.as_str() {
                    Some("minimum") => Some(StringCheckKind::Minimum),
                    Some("maximum") => Some(StringCheckKind::Maximum),
                    Some("format") => Some(StringCheckKind::Format),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default();
    match named.as_slice() {
        [first, second, third] => [*first, *second, *third],
        _ => DEFAULT,
    }
}

/// The flags of the `RegExp` behind a pattern. The JSON Schema zod lists drops them, so a validation choice
/// names them (`x-trogon-flags`); without one only the flags zod itself gives a format's regex apply (`u` for
/// emoji).
fn regexp_flags(schema: &JsonObject, format: Option<&str>) -> String {
    match schema.get("x-trogon-flags").and_then(Value::as_str) {
        Some(flags) => flags.to_owned(),
        None if format == Some("emoji") => "u".to_owned(),
        None => String::new(),
    }
}

fn number(schema: &JsonObject, integer: bool) -> Node {
    let mut min = Bound::of(schema, "minimum");
    let mut max = Bound::of(schema, "maximum");
    let mut format = "safeint";
    let (mut low, mut high) = (
        Bound::integer(-constants::JS_MAX_SAFE_INTEGER),
        Bound::integer(constants::JS_MAX_SAFE_INTEGER),
    );
    if integer && !schema.contains_key("x-trogon-bounded-integer") {
        let bounds = (
            min.as_ref().map(|bound| bound.value),
            max.as_ref().map(|bound| bound.value),
        );
        if bounds == (Some(-2_147_483_648.0), Some(2_147_483_647.0)) {
            format = "int32";
            low = Bound::integer(-2_147_483_648);
            high = Bound::integer(2_147_483_647);
        } else if bounds == (Some(0.0), Some(4_294_967_295.0)) {
            format = "uint32";
            low = Bound::integer(0);
            high = Bound::integer(4_294_967_295);
        }
        if min.as_ref().is_some_and(|bound| bound.value <= low.value) {
            min = None;
        }
        if max.as_ref().is_some_and(|bound| bound.value >= high.value) {
            max = None;
        }
    }
    Node::Numeric(NumberNode {
        integer,
        format,
        low,
        high,
        min,
        exclusive_min: Bound::of(schema, "exclusiveMinimum"),
        max,
        exclusive_max: Bound::of(schema, "exclusiveMaximum"),
        multiple_of: Bound::of(schema, "multipleOf"),
    })
}

fn array(schema: &JsonObject, callbacks: Callbacks<'_>) -> Node {
    if let Some(items) = schema.get("items").and_then(Value::as_array) {
        return Node::Tuple {
            items: items.iter().map(|item| build(item, true, callbacks)).collect(),
            rest: schema
                .get("additionalItems")
                .filter(|rest| rest.is_object())
                .map(|rest| Box::new(build(rest, true, callbacks))),
        };
    }
    let min = Bound::of(schema, "minItems");
    let max = Bound::of(schema, "maxItems");
    let exact = is_exact(min.as_ref(), max.as_ref(), schema);
    let element = match schema.get("items") {
        Some(items) if items.is_object() => build(items, true, callbacks),
        _ => build(&Value::Object(JsonObject::new()), true, callbacks),
    };
    Node::Array(Box::new(ArrayNode {
        element,
        min,
        max,
        exact,
    }))
}

fn object(schema: &JsonObject, callbacks: Callbacks<'_>) -> Node {
    let required: Vec<&str> = schema
        .get("required")
        .and_then(Value::as_array)
        .map(|names| names.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let names = schema.get("propertyNames").and_then(Value::as_object);
    let additional = schema.get("additionalProperties");
    if let (Some(names), Some(Value::Object(value)), None) = (names, additional, schema.get("properties")) {
        let value = Value::Object(value.clone());
        if let (Some(keys), Some(_)) = (
            names.get("enum").and_then(Value::as_array),
            schema.get("required").and_then(Value::as_array),
        ) {
            return Node::RecordEnum(
                keys.iter()
                    .filter_map(Value::as_str)
                    .map(|key| (key.to_owned(), build(&value, required.contains(&key), callbacks)))
                    .collect(),
            );
        }
        return Node::Record {
            key: Box::new(modeled(&Value::Object(names.clone()), callbacks)),
            value: Box::new(build(&value, true, callbacks)),
        };
    }
    let properties = match schema.get("properties").and_then(Value::as_object) {
        Some(properties) => ordered_keys(properties)
            .into_iter()
            .filter_map(|key| {
                let is_required = required.contains(&key.as_str());
                properties
                    .get(key)
                    .map(|property| (key.clone(), build(property, is_required, callbacks), is_required))
            })
            .collect(),
        None => Vec::new(),
    };
    let mode = match additional {
        Some(Value::Bool(false)) => ObjectMode::Strict,
        Some(Value::Object(catchall)) if catchall.is_empty() => ObjectMode::Catchall(Node::Any),
        Some(catchall @ Value::Object(_)) => ObjectMode::Catchall(build(catchall, true, callbacks)),
        _ => ObjectMode::Strip,
    };
    Node::Object(ObjectNode { properties, mode })
}

/// `CallToolResultSchema` of the bundle, as the JSON Schema `zod` itself derives from it (with the one
/// base64 refinement marked `x-refine`).
static CALL_TOOL_RESULT: std::sync::LazyLock<Value> = std::sync::LazyLock::new(|| {
    serde_json::from_str(include_str!("sdk_mcp_call_tool_result.json")).unwrap_or(Value::Null)
});

static CALL_TOOL_RESULT_NODE: std::sync::LazyLock<Node> =
    std::sync::LazyLock::new(|| Node::from_schema(&CALL_TOOL_RESULT, &[]));

static META_NODE: std::sync::LazyLock<Node> = std::sync::LazyLock::new(|| {
    Node::from_schema(
        CALL_TOOL_RESULT
            .get("properties")
            .and_then(|properties| properties.get("_meta"))
            .unwrap_or(&Value::Null),
        &[],
    )
});

/// The schema a handler result must satisfy.
pub(crate) fn call_tool_result() -> &'static Node {
    &CALL_TOOL_RESULT_NODE
}

/// The schema of `params._meta`: a loose object with a `progressToken` and the related task.
pub(crate) fn meta() -> &'static Node {
    &META_NODE
}

/// The `SyntaxError` text of the first `pattern` anywhere in the schema that `new RegExp` rejects.
pub(crate) fn pattern_error(schema: &Value) -> Option<String> {
    match schema {
        Value::Object(object) => {
            let own = object.get("pattern").and_then(Value::as_str).and_then(|pattern| {
                let format = object.get("format").and_then(Value::as_str);
                sdk_mcp_regexp::compile(pattern, &regexp_flags(object, format)).err()
            });
            own.or_else(|| object.values().find_map(pattern_error))
        }
        Value::Array(items) => items.iter().find_map(pattern_error),
        _ => None,
    }
}

/// Whether a tool schema uses a keyword a zod schema cannot express, so only the JSON Schema validator
/// can judge it.
pub(crate) fn has_unmodeled(schema: &JsonObject) -> bool {
    const KEYWORDS: [&str; 16] = [
        "allOf",
        "oneOf",
        "$ref",
        "$defs",
        "definitions",
        "if",
        "then",
        "else",
        "patternProperties",
        "contains",
        "uniqueItems",
        "minProperties",
        "maxProperties",
        "dependentRequired",
        "dependentSchemas",
        "prefixItems",
    ];
    fn scan(value: &Value) -> bool {
        match value {
            Value::Object(object) => object
                .iter()
                .any(|(key, value)| KEYWORDS.contains(&key.as_str()) || scan(value)),
            Value::Array(items) => items.iter().any(scan),
            _ => false,
        }
    }
    schema
        .iter()
        .any(|(key, value)| KEYWORDS.contains(&key.as_str()) || scan(value))
}

#[cfg(test)]
#[path = "sdk_mcp_zod_tests.rs"]
mod tests;
