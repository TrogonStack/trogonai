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
//! invisible. The Rust regular expression and URL engines stand in for the JavaScript ones.

use crate::{JsonObject, constants, sdk_mcp_js::ordered_keys, sdk_mcp_zod_issue::Issue};
use serde_json::{Number, Value};

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
        matcher: Option<jsonschema::Validator>,
    },
}

pub(crate) struct StringNode {
    pub(crate) min: Option<Bound>,
    pub(crate) max: Option<Bound>,
    pub(crate) exact: bool,
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
    Text(StringNode),
    Numeric(NumberNode),
    Array(Box<ArrayNode>),
    Tuple { items: Vec<Node>, rest: Option<Box<Node>> },
    Object(ObjectNode),
    Record { key: Box<Node>, value: Box<Node> },
    RecordEnum(Vec<(String, Node)>),
}

/// The result of a parse: the issues in zod's order, and the parsed value (`None` is `undefined`).
pub(crate) struct Parsed {
    pub(crate) issues: Vec<JsonObject>,
    pub(crate) value: Option<Value>,
}

impl Node {
    /// The zod schema a JSON Schema document stands for.
    pub(crate) fn from_schema(schema: &Value) -> Self {
        inner(schema)
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

fn build(schema: &Value, required: bool) -> Node {
    let node = inner(schema);
    match schema.as_object().and_then(|object| object.get("default")) {
        Some(default) if required => Node::Catch(Box::new(node), default.clone()),
        Some(default) => Node::Default(Box::new(node), default.clone()),
        None if required => node,
        None => Node::Optional(Box::new(node)),
    }
}

fn inner(schema: &Value) -> Node {
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
        if let [first, second] = options.as_slice()
            && second
                .as_object()
                .is_some_and(|only| only.len() == 1 && only.get("type").and_then(Value::as_str) == Some("null"))
        {
            return Node::Nullable(Box::new(build(first, true)));
        }
        return Node::Union(options.iter().map(|option| build(option, true)).collect());
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
            inner(&Value::Object(copy))
        };
        if let [first, second] = types.as_slice()
            && second.as_str() == Some("null")
        {
            return Node::Nullable(Box::new(member(first)));
        }
        return Node::Union(types.iter().map(member).collect());
    }
    match object.get("type").and_then(Value::as_str) {
        Some("string") => string(object),
        Some("number") => number(object, false),
        Some("integer") => number(object, true),
        Some("boolean") => Node::Boolean,
        Some("null") => Node::Null,
        Some("array") => array(object),
        Some("object") => self::object(object),
        _ => Node::Any,
    }
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
    let exact = matches!((&min, &max), (Some(low), Some(high)) if low.value == high.value);
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
            (format, Some(pattern)) => StringCheck::Pattern {
                source: pattern.to_owned(),
                format: match format {
                    None => "regex",
                    Some("date-time") => "datetime",
                    Some(other) => other,
                }
                .to_owned(),
                matcher: matcher(pattern),
            },
            _ => StringCheck::None,
        };
    }
    Node::Text(StringNode {
        min,
        max,
        exact,
        check,
        base64: schema.get("x-refine").and_then(Value::as_str) == Some("atob"),
    })
}

/// `new RegExp(source).test`, through the ECMA 262 pattern engine of the JSON Schema validator.
fn matcher(source: &str) -> Option<jsonschema::Validator> {
    let schema = JsonObject::from_iter([("pattern".to_owned(), Value::from(source))]);
    jsonschema::validator_for(&Value::Object(schema)).ok()
}

fn number(schema: &JsonObject, integer: bool) -> Node {
    let mut min = Bound::of(schema, "minimum");
    let mut max = Bound::of(schema, "maximum");
    let mut format = "safeint";
    let (mut low, mut high) = (
        Bound::integer(-constants::JS_MAX_SAFE_INTEGER),
        Bound::integer(constants::JS_MAX_SAFE_INTEGER),
    );
    if integer {
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

fn array(schema: &JsonObject) -> Node {
    if let Some(items) = schema.get("items").and_then(Value::as_array) {
        return Node::Tuple {
            items: items.iter().map(|item| build(item, true)).collect(),
            rest: schema
                .get("additionalItems")
                .filter(|rest| rest.is_object())
                .map(|rest| Box::new(build(rest, true))),
        };
    }
    let min = Bound::of(schema, "minItems");
    let max = Bound::of(schema, "maxItems");
    let exact = matches!((&min, &max), (Some(low), Some(high)) if low.value == high.value);
    let element = match schema.get("items") {
        Some(items) if items.is_object() => build(items, true),
        _ => build(&Value::Object(JsonObject::new()), true),
    };
    Node::Array(Box::new(ArrayNode {
        element,
        min,
        max,
        exact,
    }))
}

fn object(schema: &JsonObject) -> Node {
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
                    .map(|key| (key.to_owned(), build(&value, required.contains(&key))))
                    .collect(),
            );
        }
        return Node::Record {
            key: Box::new(inner(&Value::Object(names.clone()))),
            value: Box::new(build(&value, true)),
        };
    }
    let properties = match schema.get("properties").and_then(Value::as_object) {
        Some(properties) => ordered_keys(properties)
            .into_iter()
            .filter_map(|key| {
                let is_required = required.contains(&key.as_str());
                properties
                    .get(key)
                    .map(|property| (key.clone(), build(property, is_required), is_required))
            })
            .collect(),
        None => Vec::new(),
    };
    let mode = match additional {
        Some(Value::Bool(false)) => ObjectMode::Strict,
        Some(Value::Object(catchall)) if catchall.is_empty() => ObjectMode::Catchall(Node::Any),
        Some(catchall @ Value::Object(_)) => ObjectMode::Catchall(build(catchall, true)),
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
    std::sync::LazyLock::new(|| Node::from_schema(&CALL_TOOL_RESULT));

static META_NODE: std::sync::LazyLock<Node> = std::sync::LazyLock::new(|| {
    Node::from_schema(
        CALL_TOOL_RESULT
            .get("properties")
            .and_then(|properties| properties.get("_meta"))
            .unwrap_or(&Value::Null),
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
