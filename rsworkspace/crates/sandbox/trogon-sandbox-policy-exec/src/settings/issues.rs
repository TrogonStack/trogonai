//! Every failing option of a settings document at once, in the order of the
//! schema and in the wording of [`issue_wording`].

use serde_json::{Map, Value};

use super::constants::{MAX_STEPS, SCHEMA_ORDER};
use super::{SandboxSettings, SettingsError, dropped_key, remove_key, without_indices};
use super::{issue_wording, refine};

#[derive(Debug, Clone, PartialEq, Eq)]
enum Segment {
    Key(String),
    Index(usize),
}

fn segments(path: &str) -> Vec<Segment> {
    if path == "." {
        return Vec::new();
    }
    let mut found = Vec::new();
    for token in path.split('.') {
        let (name, mut rest) = token.split_once('[').map_or((token, ""), |(n, r)| (n, r));
        if !name.is_empty() {
            found.push(Segment::Key(name.to_owned()));
        }
        while let Some((index, tail)) = rest.split_once(']') {
            if let Ok(index) = index.parse() {
                found.push(Segment::Index(index));
            }
            rest = tail.strip_prefix('[').unwrap_or(tail);
        }
    }
    found
}

fn path_text(segments: &[Segment]) -> String {
    let mut text = String::new();
    for segment in segments {
        match segment {
            Segment::Key(key) => {
                if !text.is_empty() {
                    text.push('.');
                }
                text.push_str(key);
            }
            Segment::Index(index) => text.push_str(&format!("[{index}]")),
        }
    }
    if text.is_empty() { "(root)".to_owned() } else { text }
}

fn rank(segments: &[Segment]) -> Vec<(usize, usize)> {
    let mut container = String::new();
    let mut rank = Vec::with_capacity(segments.len());
    for segment in segments {
        match segment {
            Segment::Key(key) => {
                let keys = SCHEMA_ORDER
                    .iter()
                    .find(|(name, _)| *name == container)
                    .map(|(_, keys)| *keys);
                let position = keys
                    .and_then(|keys| keys.iter().position(|known| known == key))
                    .unwrap_or(usize::MAX - 1);
                rank.push((position, 0));
                if !container.is_empty() {
                    container.push('.');
                }
                container.push_str(key);
            }
            Segment::Index(index) => {
                rank.push((*index, 0));
                container.push_str("[]");
            }
        }
    }
    debug_assert_eq!(container, without_indices(&container));
    rank
}

fn node_mut<'a>(document: &'a mut Value, segments: &[Segment]) -> Option<&'a mut Value> {
    let mut node = document;
    for segment in segments {
        node = match segment {
            Segment::Key(key) => node.get_mut(key.as_str())?,
            Segment::Index(index) => node.get_mut(*index)?,
        };
    }
    Some(node)
}

fn replace(document: &mut Value, segments: &[Segment], value: Value) -> bool {
    let Some((last, parent)) = segments.split_last() else {
        *document = value;
        return true;
    };
    let Some(parent) = node_mut(document, parent) else {
        return false;
    };
    match last {
        Segment::Key(key) => parent
            .as_object_mut()
            .map(|object| object.insert(key.clone(), value))
            .is_some(),
        Segment::Index(index) => parent.get_mut(*index).map(|slot| *slot = value).is_some(),
    }
}

fn candidates() -> [Value; 12] {
    [
        Value::Array(Vec::new()),
        Value::Object(Map::new()),
        Value::String("a.com".to_owned()),
        Value::String("/x".to_owned()),
        Value::Bool(false),
        Value::from(0),
        Value::from(1),
        Value::Bool(true),
        Value::String("x".to_owned()),
        Value::String("deny".to_owned()),
        Value::String("jwt".to_owned()),
        Value::Array(vec![Value::from(1), Value::from(2)]),
    ]
}

fn quoted(message: &str, prefix: &str) -> Option<String> {
    Some(message.strip_prefix(prefix)?.split('`').next()?.to_owned())
}

fn clean(message: String) -> String {
    message.split(" at line ").next().unwrap_or(&message).to_owned()
}

fn serde_message(message: &str) -> String {
    if message.starts_with("invalid length ") && message.contains(", expected struct ") {
        return issue_wording::aborting("Expected object, received array");
    }
    let Some((received, expected)) = message
        .strip_prefix("invalid type: ")
        .and_then(|rest| rest.rsplit_once(", expected "))
    else {
        return message.to_owned();
    };
    let received = match received.split([' ', '`']).next().unwrap_or("") {
        "string" => "string",
        "integer" | "floating" => "number",
        "boolean" => "boolean",
        "null" | "unit" => "null",
        "sequence" => "array",
        "map" => "object",
        _ => return message.to_owned(),
    };
    let expected = match expected {
        "a sequence" => "array",
        "a number" => "number",
        "a string" | "a borrowed string" => "string",
        "a boolean" => "boolean",
        "a map" => "object",
        other if other.starts_with("struct ") => "object",
        "u8" | "u16" | "u32" | "u64" | "usize" | "i8" | "i16" | "i32" | "i64" | "f32" | "f64" => "number",
        _ => return message.to_owned(),
    };
    issue_wording::aborting(format!("Expected {expected}, received {received}"))
}

struct Found {
    segments: Vec<Segment>,
    anchor: Vec<Segment>,
    message: String,
    kind: issue_wording::IssueKind,
}

fn effective(err: &serde_path_to_error::Error<serde_json::Error>) -> (Vec<Segment>, String) {
    let message = clean(err.inner().to_string());
    let mut path = segments(&err.path().to_string());
    if let Some(field) = message
        .strip_prefix("missing field `")
        .and_then(|rest| rest.strip_suffix('`'))
    {
        path.push(Segment::Key(field.to_owned()));
        return (path, issue_wording::aborting("Required"));
    }
    (path, serde_message(&message))
}

fn advance(document: &mut Value, target: &[Segment]) -> bool {
    for candidate in candidates() {
        let mut trial = document.clone();
        if !replace(&mut trial, target, candidate) {
            return false;
        }
        let moved = match serde_path_to_error::deserialize::<_, SandboxSettings>(&trial) {
            Ok(_) => true,
            Err(err) => effective(&err).0 != target,
        };
        if moved {
            *document = trial;
            return true;
        }
    }
    false
}

/// Every failing option of `json`, joined as the schema orders them, or `None`
/// when the document cannot be walked option by option.
pub(super) fn report(json: &str, lenient: bool) -> Option<SettingsError> {
    let document: Value = serde_json::from_str(json).ok()?;
    report_value(document, lenient)
}

pub(super) fn report_value(mut document: Value, lenient: bool) -> Option<SettingsError> {
    let original = document.clone();
    let mut found: Vec<Found> = Vec::new();
    let mut unrecognized: Vec<(Vec<Segment>, Vec<String>)> = Vec::new();
    let silenced = |found: &[Found], path: &[Segment]| {
        found
            .iter()
            .any(|issue| path.starts_with(&issue.segments) || issue.segments.starts_with(path))
    };
    for _ in 0..MAX_STEPS {
        let err = match serde_path_to_error::deserialize::<_, SandboxSettings>(&document) {
            Ok(_) => break,
            Err(err) => err,
        };
        if lenient
            && let Some((container, key)) = dropped_key(&err)
            && remove_key(&mut document, &container, &key)
        {
            continue;
        }
        let message = clean(err.inner().to_string());
        if let Some(key) = quoted(&message, "unknown field `") {
            let mut container = segments(&err.path().to_string());
            if container.last() == Some(&Segment::Key(key.clone())) {
                container.pop();
            }
            if !silenced(&found, &container) {
                match unrecognized.iter_mut().find(|(path, _)| *path == container) {
                    Some((_, keys)) => keys.push(key.clone()),
                    None => unrecognized.push((container.clone(), vec![key.clone()])),
                }
            }
            let removed = node_mut(&mut document, &container)
                .and_then(Value::as_object_mut)
                .is_some_and(|object| object.remove(&key).is_some());
            if !removed {
                break;
            }
            continue;
        }
        let (path, text) = effective(&err);
        if !silenced(&found, &path) {
            for decoded in issue_wording::decode(&text) {
                let mut segments = path.clone();
                segments.extend(self::segments(&decoded.path));
                found.push(Found {
                    segments,
                    anchor: path.clone(),
                    message: decoded.message,
                    kind: decoded.kind,
                });
            }
        }
        if !advance(&mut document, &path) {
            break;
        }
    }
    let mut issues: Vec<(Vec<(usize, usize)>, SettingsError)> = found
        .iter()
        .map(|issue| {
            (
                rank(&issue.anchor),
                SettingsError::new(path_text(&issue.segments), issue.message.clone()),
            )
        })
        .collect();
    for (container, keys) in &unrecognized {
        let mut order = rank(container);
        order.push((usize::MAX, 0));
        let listed: Vec<String> = keys.iter().map(|key| format!("'{key}'")).collect();
        issues.push((
            order,
            SettingsError::new(
                path_text(container),
                format!("Unrecognized key(s) in object: {}", listed.join(", ")),
            ),
        ));
    }
    issues.sort_by(|a, b| a.0.cmp(&b.0));
    let mut ordered: Vec<SettingsError> = issues.into_iter().map(|(_, issue)| issue).collect();
    if !found.iter().any(|issue| issue.kind == issue_wording::IssueKind::Aborts) {
        ordered.extend(refine::issues(&original));
    }
    refine::chain(ordered)
}
