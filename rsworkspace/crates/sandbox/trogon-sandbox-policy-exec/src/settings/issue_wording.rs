//! The wording of the issues this reader reports for a refused value: the
//! kind of value expected, the one received, and the bounds it broke.
//! A message that stands for several issues of one option is encoded with
//! [`encode`] and read back by `issues.rs`.

use std::fmt;

use serde::Deserialize;
use serde::de::{self, Deserializer, IgnoredAny, MapAccess, SeqAccess, Visitor};

use super::constants::{ABORTS, ISSUE_END, PATH_END};

/// Whether the document-wide checks keep running after the issue: a
/// value of the wrong kind, a missing one, or one outside an enum or union
/// leaves nothing for them to read.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(super) enum IssueKind {
    Aborts,
    Checked,
}

pub(super) struct Decoded {
    pub path: String,
    pub kind: IssueKind,
    pub message: String,
}

/// Marks a message as an issue that stops the document-wide checks.
pub(super) fn aborting(message: impl fmt::Display) -> String {
    format!("{ABORTS}{message}")
}

/// Several issues of one option, each under a path relative to it.
pub(super) fn encode(issues: &[(&str, &str)]) -> String {
    issues
        .iter()
        .map(|(path, message)| format!("{path}{PATH_END}{message}"))
        .collect::<Vec<_>>()
        .join(&ISSUE_END.to_string())
}

pub(super) fn plain(message: &str) -> String {
    message.replace(ABORTS, "")
}

pub(super) fn decode(message: &str) -> Vec<Decoded> {
    message
        .split(ISSUE_END)
        .map(|issue| {
            let (path, text) = issue.split_once(PATH_END).unwrap_or(("", issue));
            let (kind, message) = match text.strip_prefix(ABORTS) {
                Some(message) => (IssueKind::Aborts, message),
                None => (IssueKind::Checked, text),
            };
            Decoded {
                path: path.to_owned(),
                kind,
                message: message.to_owned(),
            }
        })
        .collect()
}

pub(super) fn too_small(min: i64) -> String {
    format!("Number must be greater than or equal to {min}")
}

pub(super) fn too_big(max: i64) -> String {
    format!("Number must be less than or equal to {max}")
}

pub(super) enum Loose {
    Number(f64),
    Text(String),
    Other(&'static str),
}

impl Loose {
    pub(super) fn kind(&self) -> &'static str {
        match self {
            Self::Number(_) => "number",
            Self::Text(_) => "string",
            Self::Other(kind) => kind,
        }
    }
}

impl<'de> Deserialize<'de> for Loose {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Any;

        impl<'de> Visitor<'de> for Any {
            type Value = Loose;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("any value")
            }

            fn visit_i64<E: de::Error>(self, v: i64) -> Result<Loose, E> {
                Ok(Loose::Number(v as f64))
            }

            fn visit_u64<E: de::Error>(self, v: u64) -> Result<Loose, E> {
                Ok(Loose::Number(v as f64))
            }

            fn visit_f64<E: de::Error>(self, v: f64) -> Result<Loose, E> {
                Ok(Loose::Number(v))
            }

            fn visit_str<E: de::Error>(self, v: &str) -> Result<Loose, E> {
                Ok(Loose::Text(v.to_owned()))
            }

            fn visit_bool<E: de::Error>(self, _: bool) -> Result<Loose, E> {
                Ok(Loose::Other("boolean"))
            }

            fn visit_unit<E: de::Error>(self) -> Result<Loose, E> {
                Ok(Loose::Other("null"))
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Loose, A::Error> {
                while seq.next_element::<IgnoredAny>()?.is_some() {}
                Ok(Loose::Other("array"))
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Loose, A::Error> {
                while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
                Ok(Loose::Other("object"))
            }
        }

        deserializer.deserialize_any(Any)
    }
}

/// What an integer with optional bounds is told about a number, one entry
/// per failing check in the order they are declared.
pub(super) fn number_issues(value: f64, min: Option<i64>, max: Option<i64>) -> Vec<String> {
    let mut issues = Vec::new();
    if value.fract() != 0.0 {
        issues.push("Expected integer, received float".to_owned());
    }
    if let Some(min) = min.filter(|min| value < *min as f64) {
        issues.push(too_small(min));
    }
    if let Some(max) = max.filter(|max| value > *max as f64) {
        issues.push(too_big(max));
    }
    issues
}

pub(super) fn integer<'de, D: Deserializer<'de>>(
    deserializer: D,
    min: Option<i64>,
    max: Option<i64>,
) -> Result<i64, D::Error> {
    match Loose::deserialize(deserializer)? {
        Loose::Number(value) => {
            let issues = number_issues(value, min, max);
            if issues.is_empty() {
                Ok(value as i64)
            } else {
                let issues: Vec<(&str, &str)> = issues.iter().map(|message| ("", message.as_str())).collect();
                Err(de::Error::custom(encode(&issues)))
            }
        }
        other => Err(de::Error::custom(aborting(format!(
            "Expected number, received {}",
            other.kind()
        )))),
    }
}

/// One of `names`.
pub(super) fn one_of<'de, D: Deserializer<'de>, T: Copy>(deserializer: D, names: &[(&str, T)]) -> Result<T, D::Error> {
    let listed = names
        .iter()
        .map(|(name, _)| format!("'{name}'"))
        .collect::<Vec<_>>()
        .join(" | ");
    match Loose::deserialize(deserializer)? {
        Loose::Text(text) => names
            .iter()
            .find(|(name, _)| *name == text)
            .map(|(_, value)| *value)
            .ok_or_else(|| {
                de::Error::custom(aborting(format!(
                    "Invalid enum value. Expected {listed}, received '{text}'"
                )))
            }),
        other => Err(de::Error::custom(aborting(format!(
            "Expected {listed}, received {}",
            other.kind()
        )))),
    }
}

pub(super) fn uuid_shaped(text: &str) -> bool {
    let groups: Vec<&str> = text.split('-').collect();
    groups.len() == 5
        && groups
            .iter()
            .zip([8, 4, 4, 4, 12])
            .all(|(group, len)| group.len() == len && group.bytes().all(|b| b.is_ascii_hexdigit()))
}
