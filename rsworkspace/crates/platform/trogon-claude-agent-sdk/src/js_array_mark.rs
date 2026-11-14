//! The private-use code point that marks, inside a settings value, what a JavaScript array holds beyond JSON.

use serde_json::{Map, Value};

/// A private-use code point no input of the resolve holds. A run of holes or `undefined` elements of an array is an
/// object holding only the mark as its key, set to the run's length, and the named properties of an array sit in an
/// object after its last element that holds the mark twice as a key, set to `null`, beside them.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ArrayMark(char);

impl ArrayMark {
    /// The mark outside a resolve that keeps strings, which reserves none: the last private-use code point.
    pub(crate) const FALLBACK: Self = Self('\u{10FFFD}');

    pub(crate) const fn new(mark: char) -> Self {
        Self(mark)
    }

    fn run_key(self) -> String {
        self.0.to_string()
    }

    fn props_key(self) -> String {
        [self.0, self.0].iter().collect()
    }

    /// A run of `length` holes or `undefined` elements.
    pub(crate) fn run(self, length: u64) -> Value {
        Value::Object(Map::from_iter([(self.run_key(), Value::from(length))]))
    }

    /// The length of the run `value` stands for, if it stands for one.
    pub(crate) fn run_length(self, value: &Value) -> Option<u64> {
        match value {
            Value::Object(entries) if entries.len() == 1 => entries.get(&self.run_key()).and_then(Value::as_u64),
            _ => None,
        }
    }

    /// An empty holder of named properties.
    pub(crate) fn props(self) -> Value {
        Value::Object(Map::from_iter([(self.props_key(), Value::Null)]))
    }

    /// Whether `value` holds the named properties of an array.
    pub(crate) fn is_props(self, value: &Value) -> bool {
        matches!(value, Value::Object(entries) if entries.contains_key(&self.props_key()))
    }

    /// Whether `key` is the key that marks a holder of named properties.
    pub(crate) fn is_props_key(self, key: &str) -> bool {
        key == self.props_key()
    }
}
