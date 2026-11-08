use std::collections::{BTreeMap, HashMap};

use cel_interpreter::Value;
use cel_interpreter::objects::Key;

use crate::cel_limits::CelLimits;
use crate::constants::{MAX_FACT_NESTING, SCALAR_BYTES};
use crate::facts_limit_error::FactsLimitError;

/// One value a rule can read.
///
/// The set is small on purpose: what a gateway, a runtime or a test knows about
/// a request fits in nulls, booleans, integers, text, lists and maps. There is
/// no float, because a rule that compares floats from an untrusted source
/// compares NaN, and there is no byte string or time, because a producer that
/// has one can say it as text.
#[derive(Debug, Clone, PartialEq)]
pub enum FactValue {
    /// No value.
    Null,
    /// A boolean.
    Bool(bool),
    /// A signed integer.
    Int(i64),
    /// Text.
    String(String),
    /// An ordered list.
    List(Vec<FactValue>),
    /// A map from text to values.
    Map(BTreeMap<String, FactValue>),
}

impl FactValue {
    /// Builds a list from anything that converts to a value.
    pub fn list<T: Into<FactValue>>(items: impl IntoIterator<Item = T>) -> Self {
        Self::List(items.into_iter().map(Into::into).collect())
    }

    /// Builds a map from keys and anything that converts to a value.
    pub fn map<K: Into<String>, V: Into<FactValue>>(entries: impl IntoIterator<Item = (K, V)>) -> Self {
        Self::Map(entries.into_iter().map(|(k, v)| (k.into(), v.into())).collect())
    }

    /// Adds this value's size to `bytes`, refusing it when it is too large,
    /// too long in any collection, or nested too deep.
    pub(crate) fn charge(&self, limits: &CelLimits, depth: usize, bytes: &mut usize) -> Result<(), FactsLimitError> {
        if depth > MAX_FACT_NESTING {
            return Err(FactsLimitError::NestedTooDeep {
                limit: MAX_FACT_NESTING,
            });
        }
        match self {
            Self::Null | Self::Bool(_) | Self::Int(_) => charge_bytes(bytes, SCALAR_BYTES, limits)?,
            Self::String(text) => charge_bytes(bytes, SCALAR_BYTES.saturating_add(text.len()), limits)?,
            Self::List(items) => {
                check_len(items.len(), limits)?;
                charge_bytes(bytes, SCALAR_BYTES, limits)?;
                for item in items {
                    item.charge(limits, depth + 1, bytes)?;
                }
            }
            Self::Map(entries) => {
                check_len(entries.len(), limits)?;
                charge_bytes(bytes, SCALAR_BYTES, limits)?;
                for (key, value) in entries {
                    charge_bytes(bytes, key.len(), limits)?;
                    value.charge(limits, depth + 1, bytes)?;
                }
            }
        }
        Ok(())
    }

    /// Converts to the interpreter's value. Call after [`Self::charge`] has
    /// accepted the value, which bounds the recursion.
    pub(crate) fn to_cel(&self) -> Value {
        match self {
            Self::Null => Value::Null,
            Self::Bool(value) => Value::Bool(*value),
            Self::Int(value) => Value::Int(*value),
            Self::String(value) => Value::from(value.as_str()),
            Self::List(items) => Value::from(items.iter().map(Self::to_cel).collect::<Vec<_>>()),
            Self::Map(entries) => Value::from(
                entries
                    .iter()
                    .map(|(key, value)| (Key::from(key.as_str()), value.to_cel()))
                    .collect::<HashMap<_, _>>(),
            ),
        }
    }
}

fn charge_bytes(total: &mut usize, amount: usize, limits: &CelLimits) -> Result<(), FactsLimitError> {
    *total = total.saturating_add(amount);
    if *total > limits.max_input_bytes() {
        return Err(FactsLimitError::TooManyBytes {
            limit: limits.max_input_bytes(),
        });
    }
    Ok(())
}

fn check_len(actual: usize, limits: &CelLimits) -> Result<(), FactsLimitError> {
    if actual > limits.max_collection_len() {
        return Err(FactsLimitError::CollectionTooLong {
            actual,
            limit: limits.max_collection_len(),
        });
    }
    Ok(())
}

impl From<bool> for FactValue {
    fn from(value: bool) -> Self {
        Self::Bool(value)
    }
}

impl From<i64> for FactValue {
    fn from(value: i64) -> Self {
        Self::Int(value)
    }
}

impl From<i32> for FactValue {
    fn from(value: i32) -> Self {
        Self::Int(i64::from(value))
    }
}

impl From<u16> for FactValue {
    fn from(value: u16) -> Self {
        Self::Int(i64::from(value))
    }
}

impl From<&str> for FactValue {
    fn from(value: &str) -> Self {
        Self::String(value.to_owned())
    }
}

impl From<String> for FactValue {
    fn from(value: String) -> Self {
        Self::String(value)
    }
}

impl<T: Into<FactValue>> From<Option<T>> for FactValue {
    fn from(value: Option<T>) -> Self {
        value.map_or(Self::Null, Into::into)
    }
}

impl<T: Into<FactValue>> From<Vec<T>> for FactValue {
    fn from(value: Vec<T>) -> Self {
        Self::list(value)
    }
}
