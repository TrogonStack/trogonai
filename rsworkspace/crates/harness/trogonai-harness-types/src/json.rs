//! Whether provider-supplied text is a single complete JSON document.

use std::collections::BTreeSet;
use std::fmt;

use serde::de::{self, Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};

/// Whether `text` is one complete JSON document with no repeated object key.
#[must_use]
pub fn is_strict_document(text: &str) -> bool {
    strict_document_fault(text).is_none()
}

/// Why text failed to be one strict document, located by position only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentFault {
    /// The text ended before the value was complete.
    Truncated,
    /// Parsing stopped at the byte at `offset`.
    Syntax {
        /// Byte offset of the byte parsing stopped at.
        offset: usize,
    },
    /// The syntax was complete, but the value was refused, such as an object
    /// that repeats a key.
    RejectedValue,
}

/// The reason `text` is not one strict document, or `None` when it is one.
#[must_use]
pub fn strict_document_fault(text: &str) -> Option<DocumentFault> {
    let error = serde_json::from_str::<Walked>(text).err()?;
    Some(match error.classify() {
        serde_json::error::Category::Eof => DocumentFault::Truncated,
        serde_json::error::Category::Data => DocumentFault::RejectedValue,
        serde_json::error::Category::Syntax | serde_json::error::Category::Io => DocumentFault::Syntax {
            offset: byte_offset(text, error.line(), error.column()),
        },
    })
}

/// The offset of the byte a parser error points just past.
fn byte_offset(text: &str, line: usize, column: usize) -> usize {
    let start_of_line = match line.checked_sub(2) {
        None => 0,
        Some(preceding) => text
            .match_indices('\n')
            .nth(preceding)
            .map_or(text.len(), |(index, _)| index + 1),
    };
    (start_of_line + column).saturating_sub(1).min(text.len())
}

/// A document that was walked and not kept.
struct Walked;

impl<'de> Deserialize<'de> for Walked {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(Walk)
    }
}

struct Walk;

impl<'de> Visitor<'de> for Walk {
    type Value = Walked;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("any JSON value")
    }

    fn visit_bool<E>(self, _: bool) -> Result<Walked, E> {
        Ok(Walked)
    }

    fn visit_i64<E>(self, _: i64) -> Result<Walked, E> {
        Ok(Walked)
    }

    fn visit_u64<E>(self, _: u64) -> Result<Walked, E> {
        Ok(Walked)
    }

    fn visit_f64<E>(self, _: f64) -> Result<Walked, E> {
        Ok(Walked)
    }

    fn visit_str<E>(self, _: &str) -> Result<Walked, E> {
        Ok(Walked)
    }

    fn visit_unit<E>(self) -> Result<Walked, E> {
        Ok(Walked)
    }

    fn visit_none<E>(self) -> Result<Walked, E> {
        Ok(Walked)
    }

    fn visit_some<D>(self, deserializer: D) -> Result<Walked, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(Walk)
    }

    fn visit_seq<A>(self, mut seq: A) -> Result<Walked, A::Error>
    where
        A: SeqAccess<'de>,
    {
        while seq.next_element::<Walked>()?.is_some() {}
        Ok(Walked)
    }

    fn visit_map<A>(self, mut map: A) -> Result<Walked, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut seen = BTreeSet::new();
        while let Some(key) = map.next_key::<String>()? {
            map.next_value::<Walked>()?;
            if !seen.insert(key) {
                return Err(de::Error::custom("duplicate object key"));
            }
        }
        Ok(Walked)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_complete_document_of_any_root_type_is_accepted() {
        for text in [
            "  {\"first\":1,\"second\":1e+02} \n",
            "[1,{\"nested\":true}]",
            "null",
            "true",
            "42",
            "\"text\"",
            "{}",
        ] {
            assert!(is_strict_document(text), "{text} should parse");
        }
    }

    #[test]
    fn malformed_or_trailing_content_is_refused() {
        for text in ["{]", "{} trailing", "", "{\"a\":1", "1 2"] {
            assert!(!is_strict_document(text), "{text} should not parse");
        }
    }

    #[test]
    fn a_repeated_key_is_refused_at_any_depth() {
        assert!(!is_strict_document("{\"depth\":1,\"depth\":2}"));
        assert!(!is_strict_document("{\"outer\":{\"a\":1,\"a\":2}}"));
        assert!(!is_strict_document("[{\"a\":1,\"a\":2}]"));
        assert!(
            is_strict_document("{\"outer\":{\"a\":1},\"other\":{\"a\":2}}"),
            "the same key in sibling objects is not a duplicate"
        );
    }

    #[test]
    fn a_fault_locates_truncated_syntax_and_rejected_input() {
        let cut = "{\"request\":{\"action\":\"run\",\"task\":\"Investigate the \\\"slow exit\\\" path";
        assert_eq!(strict_document_fault(cut), Some(DocumentFault::Truncated));
        assert_eq!(strict_document_fault(""), Some(DocumentFault::Truncated));

        let unescaped = "{\"task\":\"say \"hi\" now\"}";
        assert_eq!(
            strict_document_fault(unescaped),
            Some(DocumentFault::Syntax {
                offset: unescaped.find("hi").expect("fixture")
            })
        );

        let trailing = "{\"path\":\"a\",}";
        assert_eq!(
            strict_document_fault(trailing),
            Some(DocumentFault::Syntax {
                offset: trailing.len() - 1
            })
        );

        let second_line = "{\n\"a\":1,\n\"b\" 2}";
        assert_eq!(
            strict_document_fault(second_line),
            Some(DocumentFault::Syntax {
                offset: second_line.find(" 2").expect("fixture") + 1
            })
        );

        assert_eq!(
            strict_document_fault("{\"path\":\"a\",\"path\":\"b\"}"),
            Some(DocumentFault::RejectedValue)
        );
        assert_eq!(strict_document_fault("{\"path\":\"a\"}"), None);
    }

    #[test]
    fn a_fault_offset_never_points_past_the_input() {
        for text in ["{]", "{} trailing", "1 2", "\u{ff}{", "{\"t\":\"\u{e9}\\x\"}", "[1,]"] {
            if let Some(DocumentFault::Syntax { offset }) = strict_document_fault(text) {
                assert!(offset <= text.len(), "{text} reported {offset}");
            }
        }
    }

    #[test]
    fn unbounded_nesting_is_refused_rather_than_overflowing_the_stack() {
        let deep = format!("{}1{}", "[".repeat(2048), "]".repeat(2048));
        assert!(!is_strict_document(&deep));
    }
}
