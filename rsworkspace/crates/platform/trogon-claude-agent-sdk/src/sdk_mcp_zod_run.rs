//! The parse of the zod port: the checks, issue order, abort rules and reports of zod 4.6.2.

use crate::{
    JsonObject, constants,
    sdk_mcp_choices::{SchemaCallback, StringCheckKind},
    sdk_mcp_js::{
        float_safe_remainder, forgiving_base64, js_string, ordered, ordered_keys, prototype_function_length, to_number,
        trim, utf16_length,
    },
    sdk_mcp_regexp,
    sdk_mcp_zod::{Affix, ArrayNode, Bound, Node, NumberNode, ObjectMode, ObjectNode, StringCheck, StringNode},
    sdk_mcp_zod_issue::{Issue, aborted},
};
use serde_json::Value;
use std::collections::HashSet;

/// A JavaScript value the parse reads: JSON, `undefined`, or the function an absent key named like a member of
/// `Object.prototype` reads as (its `length` is the arity of the function).
#[derive(Clone, Copy)]
pub(crate) enum Input<'a> {
    Undefined,
    Function(usize),
    Json(&'a Value),
}

/// A value the parse produces. `undefined` and functions disappear when the result is serialized.
pub(crate) enum Out {
    Undefined,
    Function,
    Json(Value),
}

impl Out {
    fn pass(input: Input<'_>) -> Self {
        match input {
            Input::Undefined => Self::Undefined,
            Input::Function(_) => Self::Function,
            Input::Json(value) => Self::Json(value.clone()),
        }
    }

    fn is_undefined(&self) -> bool {
        matches!(self, Self::Undefined)
    }

    /// The serialized value; `None` is `undefined`.
    pub(crate) fn into_json(self) -> Option<Value> {
        match self {
            Self::Json(value) => Some(value),
            Self::Undefined | Self::Function => None,
        }
    }

    /// The array element: `undefined` and functions serialize as `null`.
    fn element(self) -> Value {
        self.into_json().unwrap_or(Value::Null)
    }
}

pub(crate) struct Outcome {
    pub(crate) issues: Vec<Issue>,
    pub(crate) value: Out,
}

impl Outcome {
    fn ok(value: Value) -> Self {
        Self {
            issues: Vec::new(),
            value: Out::Json(value),
        }
    }

    fn fail(issue: Issue, input: Input<'_>) -> Self {
        Self {
            issues: vec![issue],
            value: Out::pass(input),
        }
    }

    fn pass(input: Input<'_>) -> Self {
        Self {
            issues: Vec::new(),
            value: Out::pass(input),
        }
    }
}

fn received(input: Input<'_>) -> &'static str {
    match input {
        Input::Undefined => "undefined",
        Input::Function(_) => "function",
        Input::Json(Value::Null) => "null",
        Input::Json(Value::Array(_)) => "array",
        Input::Json(Value::Object(_)) => "object",
        Input::Json(Value::String(_)) => "string",
        Input::Json(Value::Number(_)) => "number",
        Input::Json(Value::Bool(_)) => "boolean",
    }
}

fn type_issue(expected: &str, input: Input<'_>) -> Issue {
    Issue::invalid_type(expected, received(input))
}

fn prefixed(issues: Vec<Issue>, segment: &Value) -> impl Iterator<Item = Issue> {
    let segment = segment.clone();
    issues.into_iter().map(move |issue| issue.at(segment.clone()))
}

fn index(position: usize) -> Value {
    Value::from(position as u64)
}

/// `object[key]`, including the functions an absent key inherits from `Object.prototype`.
fn lookup<'a>(object: &'a JsonObject, key: &str) -> Input<'a> {
    match object.get(key) {
        Some(value) => Input::Json(value),
        None => prototype_function_length(key).map_or(Input::Undefined, Input::Function),
    }
}

/// `key in object`.
fn has(object: &JsonObject, key: &str) -> bool {
    object.contains_key(key) || prototype_function_length(key).is_some()
}

fn reports(issues: &[Issue]) -> Value {
    Value::Array(issues.iter().map(|issue| Value::Object(issue.report())).collect())
}

pub(crate) fn run(node: &Node, input: Input<'_>) -> Outcome {
    match node {
        Node::Any => Outcome::pass(input),
        Node::Never => Outcome::fail(type_issue("never", input), input),
        Node::Boolean => match input {
            Input::Json(Value::Bool(_)) => Outcome::pass(input),
            _ => Outcome::fail(type_issue("boolean", input), input),
        },
        Node::Null => match input {
            Input::Json(Value::Null) => Outcome::pass(input),
            _ => Outcome::fail(type_issue("null", input), input),
        },
        Node::Literal(values) => {
            if values.iter().any(|value| strictly_equal(value, input)) {
                Outcome::pass(input)
            } else {
                Outcome::fail(
                    Issue::new(
                        [
                            ("code", "invalid_value".into()),
                            ("values", Value::Array(values.clone())),
                        ],
                        false,
                    ),
                    input,
                )
            }
        }
        Node::Optional(inner) => match input {
            Input::Undefined => Outcome::pass(input),
            _ => run(inner, input),
        },
        Node::Default(inner, default) => match input {
            Input::Undefined => Outcome::ok(default.clone()),
            _ => {
                let result = run(inner, input);
                if result.value.is_undefined() {
                    Outcome {
                        issues: result.issues,
                        value: Out::Json(default.clone()),
                    }
                } else {
                    result
                }
            }
        },
        Node::Catch(inner, default) => {
            let result = run(inner, input);
            if result.issues.is_empty() {
                result
            } else {
                Outcome::ok(default.clone())
            }
        }
        Node::Nullable(inner) => match input {
            Input::Json(Value::Null) => Outcome::pass(input),
            _ => run(inner, input),
        },
        Node::Union(options) => union(options, input),
        Node::Discriminated { key, options } => discriminated(key, options, input),
        Node::Callback(inner, callback) => with_callback(inner, callback, input),
        Node::Text(text) => string(text, input),
        Node::Numeric(number) => numeric(number, input),
        Node::Array(array) => self::array(array, input),
        Node::Tuple { items, rest } => tuple(items, rest.as_deref(), input),
        Node::Object(object) => self::object(object, input),
        Node::Record { key, value } => record(key, value, input),
        Node::RecordEnum(keys) => record_enum(keys, input),
    }
}

/// `value === input` for a JSON literal.
fn strictly_equal(value: &Value, input: Input<'_>) -> bool {
    match (value, input) {
        (Value::Null, Input::Json(Value::Null)) => true,
        (Value::Bool(left), Input::Json(Value::Bool(right))) => left == right,
        (Value::String(left), Input::Json(Value::String(right))) => left == right,
        (Value::Number(left), Input::Json(Value::Number(right))) => left.as_f64() == right.as_f64(),
        _ => false,
    }
}

/// `z.discriminatedUnion`: the option the discriminator selects parses the value; no match is one issue at
/// the discriminator.
fn discriminated(key: &str, options: &[(Vec<Value>, Node)], input: Input<'_>) -> Outcome {
    let Input::Json(Value::Object(map)) = input else {
        return Outcome::fail(type_issue("object", input), input);
    };
    let tag = lookup(map, key);
    match options
        .iter()
        .find(|(values, _)| values.iter().any(|value| strictly_equal(value, tag)))
    {
        Some((_, option)) => run(option, input),
        None => Outcome::fail(Issue::no_matching_discriminator(key).at(Value::from(key)), input),
    }
}

/// `.refine(check)` and `.transform(map)`: the refinement runs when no earlier check aborted, and the
/// transformation runs when nothing failed.
fn with_callback(node: &Node, callback: &SchemaCallback, input: Input<'_>) -> Outcome {
    let mut result = run(node, input);
    if aborted(&result.issues) {
        return result;
    }
    if let Out::Json(value) = &result.value
        && let Err(failure) = callback.refine(value)
    {
        result.issues.push(Issue::custom(failure.message()));
    }
    if !result.issues.is_empty() {
        return result;
    }
    match result.value {
        Out::Json(value) => match callback.transform(value) {
            Ok(mapped) => Outcome::ok(mapped),
            Err(failure) => Outcome::fail(Issue::added(failure.message()), input),
        },
        other => Outcome {
            issues: result.issues,
            value: other,
        },
    }
}

fn union(options: &[Node], input: Input<'_>) -> Outcome {
    let mut results: Vec<Outcome> = Vec::new();
    for option in options {
        let result = run(option, input);
        if result.issues.is_empty() {
            return result;
        }
        results.push(result);
    }
    let open: Vec<usize> = results
        .iter()
        .enumerate()
        .filter(|(_, result)| !aborted(&result.issues))
        .map(|(position, _)| position)
        .collect();
    if let [only] = open.as_slice() {
        return results.swap_remove(*only);
    }
    let errors = results.iter().map(|result| reports(&result.issues)).collect();
    Outcome::fail(
        Issue::new(
            [("code", "invalid_union".into()), ("errors", Value::Array(errors))],
            false,
        ),
        input,
    )
}

/// The `length` of a value zod's length checks can measure, with the origin the issue names.
fn lengthable(input: Input<'_>) -> Option<(&'static str, Value)> {
    match input {
        Input::Json(Value::String(text)) => Some(("string", Value::from(utf16_length(text) as u64))),
        Input::Json(Value::Array(items)) => Some(("array", Value::from(items.len() as u64))),
        Input::Json(Value::Object(object)) => object.get("length").map(|length| ("unknown", length.clone())),
        Input::Function(arity) => Some(("unknown", Value::from(arity as u64))),
        _ => None,
    }
}

/// One length check of a string or array: the minimum, the maximum, or the exact length at the position of
/// the minimum. They run even after an earlier check aborted.
fn length_check(
    kind: StringCheckKind,
    min: Option<&Bound>,
    max: Option<&Bound>,
    exact: bool,
    input: Input<'_>,
    issues: &mut Vec<Issue>,
) {
    let Some((origin, length)) = lengthable(input) else {
        return;
    };
    let measured = to_number(&length);
    if exact {
        if kind != StringCheckKind::Minimum {
            return;
        }
        let Some(bound) = min else {
            return;
        };
        if matches!(&length, Value::Number(number) if number.as_f64() == Some(bound.value)) {
            return;
        }
        let issue = if measured > bound.value {
            Issue::new(
                [
                    ("origin", origin.into()),
                    ("code", "too_big".into()),
                    ("maximum", bound.json()),
                    ("inclusive", true.into()),
                    ("exact", true.into()),
                ],
                true,
            )
        } else {
            Issue::new(
                [
                    ("origin", origin.into()),
                    ("code", "too_small".into()),
                    ("minimum", bound.json()),
                    ("inclusive", true.into()),
                    ("exact", true.into()),
                ],
                true,
            )
        };
        issues.push(issue);
        return;
    }
    match kind {
        StringCheckKind::Minimum => {
            if let Some(bound) = min
                && !(measured >= bound.value)
            {
                issues.push(Issue::new(
                    [
                        ("origin", origin.into()),
                        ("code", "too_small".into()),
                        ("minimum", bound.json()),
                        ("inclusive", true.into()),
                    ],
                    true,
                ));
            }
        }
        StringCheckKind::Maximum => {
            if let Some(bound) = max
                && !(measured <= bound.value)
            {
                issues.push(Issue::new(
                    [
                        ("origin", origin.into()),
                        ("code", "too_big".into()),
                        ("maximum", bound.json()),
                        ("inclusive", true.into()),
                    ],
                    true,
                ));
            }
        }
        StringCheckKind::Format => {}
    }
}

/// The minimum, maximum and exact length checks in their usual order.
fn length_checks(min: Option<&Bound>, max: Option<&Bound>, exact: bool, input: Input<'_>, issues: &mut Vec<Issue>) {
    for kind in [StringCheckKind::Minimum, StringCheckKind::Maximum] {
        length_check(kind, min, max, exact, input, issues);
    }
}

/// The `pattern` or `format` check of a string, unless an earlier check aborted.
fn format_check(node: &StringNode, text: &str, issues: &mut Vec<Issue>, value: &mut Out) {
    if aborted(issues) {
        return;
    }
    match &node.check {
        StringCheck::None => {}
        StringCheck::Affix { kind, text: needle } => {
            let (name, key, matches) = match kind {
                Affix::StartsWith => ("starts_with", "prefix", text.starts_with(needle.as_str())),
                Affix::EndsWith => ("ends_with", "suffix", text.ends_with(needle.as_str())),
                Affix::Includes => ("includes", "includes", text.contains(needle.as_str())),
            };
            if !matches {
                issues.push(Issue::new(
                    [
                        ("origin", "string".into()),
                        ("code", "invalid_format".into()),
                        ("format", name.into()),
                        (key, needle.as_str().into()),
                    ],
                    true,
                ));
            }
        }
        StringCheck::Pattern {
            source,
            format,
            flags,
            matcher,
        } => {
            if let Some(matcher) = matcher
                && !sdk_mcp_regexp::test(matcher, text, flags)
            {
                issues.push(Issue::new(
                    [
                        ("origin", "string".into()),
                        ("code", "invalid_format".into()),
                        ("format", format.as_str().into()),
                        ("pattern", format!("/{source}/{flags}").into()),
                    ],
                    true,
                ));
            }
        }
        StringCheck::Url => {
            let trimmed = trim(text);
            if url::Url::parse(trimmed).is_ok() {
                *value = Out::Json(Value::from(trimmed));
            } else {
                issues.push(Issue::new(
                    [("code", "invalid_format".into()), ("format", "url".into())],
                    true,
                ));
            }
        }
    }
}

fn string(node: &StringNode, input: Input<'_>) -> Outcome {
    let mut issues = Vec::new();
    let text = match input {
        Input::Json(Value::String(text)) => Some(text.as_str()),
        _ => None,
    };
    if text.is_none() {
        issues.push(type_issue("string", input));
    }
    let mut value = Out::pass(input);
    for kind in node.order {
        match (kind, text) {
            (StringCheckKind::Format, Some(text)) => format_check(node, text, &mut issues, &mut value),
            (StringCheckKind::Format, None) => {}
            (kind, _) => length_check(
                kind,
                node.min.as_ref(),
                node.max.as_ref(),
                node.exact,
                input,
                &mut issues,
            ),
        }
    }
    if let Some(text) = text
        && node.base64
        && !aborted(&issues)
        && !forgiving_base64(text)
    {
        issues.push(Issue::custom("Invalid Base64 string"));
    }
    Outcome { issues, value }
}

fn numeric(node: &NumberNode, input: Input<'_>) -> Outcome {
    let Input::Json(Value::Number(number)) = input else {
        return Outcome::fail(type_issue("number", input), input);
    };
    let Some(value) = number.as_f64().filter(|value| value.is_finite()) else {
        return Outcome::fail(type_issue("number", input), input);
    };
    let mut issues: Vec<Issue> = Vec::new();
    if node.integer && !aborted(&issues) {
        if value.fract() != 0.0 {
            issues.push(Issue::invalid_int(node.format));
        } else if value.abs() > constants::JS_MAX_SAFE_INTEGER as f64 {
            let (code, key, limit) = if value > 0.0 {
                ("too_big", "maximum", constants::JS_MAX_SAFE_INTEGER)
            } else {
                ("too_small", "minimum", -constants::JS_MAX_SAFE_INTEGER)
            };
            issues.push(Issue::new(
                [
                    ("code", code.into()),
                    (key, Value::from(limit)),
                    ("note", "Integers must be within the safe integer range.".into()),
                    ("origin", "int".into()),
                ],
                true,
            ));
        } else {
            if value < node.low.value {
                issues.push(Issue::new(
                    [
                        ("origin", "number".into()),
                        ("code", "too_small".into()),
                        ("minimum", node.low.json()),
                        ("inclusive", true.into()),
                    ],
                    true,
                ));
            }
            if value > node.high.value {
                issues.push(Issue::new(
                    [
                        ("origin", "number".into()),
                        ("code", "too_big".into()),
                        ("maximum", node.high.json()),
                    ],
                    false,
                ));
            }
        }
    }
    let small = |bound: &Bound, inclusive: bool| {
        Issue::new(
            [
                ("origin", "number".into()),
                ("code", "too_small".into()),
                ("minimum", bound.json()),
                ("inclusive", inclusive.into()),
            ],
            true,
        )
    };
    let big = |bound: &Bound, inclusive: bool| {
        Issue::new(
            [
                ("origin", "number".into()),
                ("code", "too_big".into()),
                ("maximum", bound.json()),
                ("inclusive", inclusive.into()),
            ],
            true,
        )
    };
    if let Some(bound) = &node.min
        && !aborted(&issues)
        && !(value >= bound.value)
    {
        issues.push(small(bound, true));
    }
    if let Some(bound) = &node.exclusive_min
        && !aborted(&issues)
        && !(value > bound.value)
    {
        issues.push(small(bound, false));
    }
    if let Some(bound) = &node.max
        && !aborted(&issues)
        && !(value <= bound.value)
    {
        issues.push(big(bound, true));
    }
    if let Some(bound) = &node.exclusive_max
        && !aborted(&issues)
        && !(value < bound.value)
    {
        issues.push(big(bound, false));
    }
    if let Some(bound) = &node.multiple_of
        && !aborted(&issues)
        && float_safe_remainder(value, bound.value) != 0.0
    {
        issues.push(Issue::new(
            [
                ("origin", "number".into()),
                ("code", "not_multiple_of".into()),
                ("divisor", bound.json()),
            ],
            true,
        ));
    }
    Outcome {
        issues,
        value: Out::pass(input),
    }
}

fn array(node: &ArrayNode, input: Input<'_>) -> Outcome {
    let mut issues = Vec::new();
    let value = match input {
        Input::Json(Value::Array(items)) => {
            let mut out = Vec::with_capacity(items.len());
            for (position, item) in items.iter().enumerate() {
                let result = run(&node.element, Input::Json(item));
                issues.extend(prefixed(result.issues, &index(position)));
                out.push(result.value.element());
            }
            Out::Json(Value::Array(out))
        }
        _ => {
            issues.push(type_issue("array", input));
            Out::pass(input)
        }
    };
    length_checks(node.min.as_ref(), node.max.as_ref(), node.exact, input, &mut issues);
    Outcome { issues, value }
}

fn tuple(items: &[Node], rest: Option<&Node>, input: Input<'_>) -> Outcome {
    let Input::Json(Value::Array(values)) = input else {
        return Outcome::fail(type_issue("tuple", input), input);
    };
    if rest.is_none() {
        let big = values.len() > items.len();
        let small = values.len() + 1 < items.len();
        if big || small {
            let issue = if big {
                Issue::new(
                    [
                        ("code", "too_big".into()),
                        ("maximum", Value::from(items.len() as u64)),
                        ("origin", "array".into()),
                    ],
                    false,
                )
            } else {
                Issue::new(
                    [
                        ("code", "too_small".into()),
                        ("minimum", Value::from(items.len() as u64)),
                        ("origin", "array".into()),
                    ],
                    false,
                )
            };
            return Outcome {
                issues: vec![issue],
                value: Out::Json(Value::Array(Vec::new())),
            };
        }
    }
    let mut issues = Vec::new();
    let mut out: Vec<Value> = Vec::new();
    for (position, item) in items.iter().enumerate() {
        let element = values.get(position).map_or(Input::Undefined, Input::Json);
        let result = run(item, element);
        issues.extend(prefixed(result.issues, &index(position)));
        out.push(result.value.element());
    }
    if let Some(rest) = rest {
        for (offset, element) in values.iter().skip(items.len()).enumerate() {
            let position = items.len() + offset;
            let result = run(rest, Input::Json(element));
            issues.extend(prefixed(result.issues, &index(position)));
            out.push(result.value.element());
        }
    }
    Outcome {
        issues,
        value: Out::Json(Value::Array(out)),
    }
}

/// `target[key] = value`; an `undefined` result leaves no member once the result is serialized.
fn assign(out: &mut JsonObject, key: String, value: Out) {
    if let Some(value) = value.into_json() {
        out.insert(key, value);
    }
}

fn object(node: &ObjectNode, input: Input<'_>) -> Outcome {
    let Input::Json(Value::Object(map)) = input else {
        return Outcome::fail(type_issue("object", input), input);
    };
    let mut issues = Vec::new();
    let mut out = JsonObject::new();
    for (key, child, required) in &node.properties {
        let result = run(child, lookup(map, key));
        if *required && !has(map, key) {
            if result.issues.is_empty() {
                issues.push(Issue::invalid_type("nonoptional", "undefined").at(Value::from(key.as_str())));
            } else {
                issues.extend(prefixed(result.issues, &Value::from(key.as_str())));
            }
            continue;
        }
        issues.extend(prefixed(result.issues, &Value::from(key.as_str())));
        assign(&mut out, key.clone(), result.value);
    }
    if matches!(node.mode, ObjectMode::Strip) {
        return Outcome {
            issues,
            value: Out::Json(Value::Object(ordered(out))),
        };
    }
    let known: HashSet<&str> = node.properties.iter().map(|(key, _, _)| key.as_str()).collect();
    let mut unrecognized: Vec<Value> = Vec::new();
    for key in ordered_keys(map) {
        if known.contains(key.as_str()) {
            continue;
        }
        match &node.mode {
            ObjectMode::Catchall(_) if key == "__proto__" => {}
            ObjectMode::Catchall(catchall) => {
                let result = run(catchall, lookup(map, key));
                issues.extend(prefixed(result.issues, &Value::from(key.as_str())));
                assign(&mut out, key.clone(), result.value);
            }
            ObjectMode::Strict | ObjectMode::Strip => unrecognized.push(Value::from(key.as_str())),
        }
    }
    if !unrecognized.is_empty() {
        issues.push(Issue::new(
            [
                ("code", "unrecognized_keys".into()),
                ("keys", Value::Array(unrecognized)),
            ],
            false,
        ));
    }
    Outcome {
        issues,
        value: Out::Json(Value::Object(ordered(out))),
    }
}

fn record(key_node: &Node, value_node: &Node, input: Input<'_>) -> Outcome {
    let Input::Json(Value::Object(map)) = input else {
        return Outcome::fail(type_issue("record", input), input);
    };
    let mut issues = Vec::new();
    let mut out = JsonObject::new();
    for key in ordered_keys(map) {
        if key == "__proto__" {
            continue;
        }
        let key_value = Value::from(key.as_str());
        let checked = run(key_node, Input::Json(&key_value));
        if !checked.issues.is_empty() {
            issues.push(
                Issue::new(
                    [
                        ("code", "invalid_key".into()),
                        ("origin", "record".into()),
                        ("issues", reports(&checked.issues)),
                    ],
                    false,
                )
                .at(key_value),
            );
            if let Some(name) = checked.value.into_json() {
                out.insert(js_string(&name), name);
            }
            continue;
        }
        let result = run(value_node, lookup(map, key));
        issues.extend(prefixed(result.issues, &Value::from(key.as_str())));
        if let Some(name) = checked.value.into_json() {
            assign(&mut out, js_string(&name), result.value);
        }
    }
    Outcome {
        issues,
        value: Out::Json(Value::Object(ordered(out))),
    }
}

fn record_enum(keys: &[(String, Node)], input: Input<'_>) -> Outcome {
    let Input::Json(Value::Object(map)) = input else {
        return Outcome::fail(type_issue("record", input), input);
    };
    let mut issues = Vec::new();
    let mut out = JsonObject::new();
    for (key, child) in keys {
        let result = run(child, lookup(map, key));
        issues.extend(prefixed(result.issues, &Value::from(key.as_str())));
        assign(&mut out, key.clone(), result.value);
    }
    let unrecognized: Vec<Value> = ordered_keys(map)
        .into_iter()
        .filter(|key| !keys.iter().any(|(name, _)| name == *key))
        .map(|key| Value::from(key.as_str()))
        .collect();
    if !unrecognized.is_empty() {
        issues.push(Issue::new(
            [
                ("code", "unrecognized_keys".into()),
                ("keys", Value::Array(unrecognized)),
            ],
            false,
        ));
    }
    Outcome {
        issues,
        value: Out::Json(Value::Object(ordered(out))),
    }
}
