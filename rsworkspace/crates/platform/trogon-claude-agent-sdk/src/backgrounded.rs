use serde_json::Value;

/// What TypeScript `backgroundTasks` returns: `response.backgrounded ?? true`, of whatever type the CLI sent.
#[derive(Clone, Debug, PartialEq)]
pub enum Backgrounded {
    Bool(bool),
    /// A `backgrounded` of another type, exactly as the CLI sent it.
    Other(Value),
}

impl Backgrounded {
    /// `response.backgrounded ?? true` over the `response` object of a `background_tasks` reply.
    pub(crate) fn from_response(value: Option<&Value>) -> Self {
        match value {
            None | Some(Value::Null) => Self::Bool(true),
            Some(Value::Bool(backgrounded)) => Self::Bool(*backgrounded),
            Some(other) => Self::Other(other.clone()),
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(backgrounded) => Some(*backgrounded),
            Self::Other(_) => None,
        }
    }

    /// The opt-in boolean check: a `backgrounded` of another type is a [`BackgroundedError`].
    pub fn into_bool(self) -> Result<bool, BackgroundedError> {
        match self {
            Self::Bool(backgrounded) => Ok(backgrounded),
            Self::Other(value) => Err(BackgroundedError { value }),
        }
    }
}

/// A `background_tasks` reply whose `backgrounded` is not a boolean.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
#[error("background_tasks answered a backgrounded that is not a boolean")]
pub struct BackgroundedError {
    value: Value,
}

impl BackgroundedError {
    /// The value the CLI sent.
    pub fn value(&self) -> &Value {
        &self.value
    }
}
