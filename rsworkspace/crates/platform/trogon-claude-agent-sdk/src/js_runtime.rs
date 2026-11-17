/// The JavaScript runtime the TypeScript SDK runs under. Its engine words the errors the SDK raises without
/// writing them itself, such as the `TypeError` from `"method" in message` when the CLI sends an `mcp_message`
/// whose `message` is not an object.
///
/// `Node`, the default, reproduces V8 (`Cannot use 'in' operator to search for 'method' in 7`). `Bun` reproduces
/// JavaScriptCore, whose text names the bundled parameter instead of the value
/// (`e is not an Object. (evaluating '"method" in e')`). The Python dialect raises no such error.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum JsRuntime {
    #[default]
    Node,
    Bun,
}

/// A JavaScript value without properties: reading one off it throws.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Nullish {
    Undefined,
    Null,
}

impl Nullish {
    /// `undefined` for a missing value, `null` for JSON `null`, `None` for a value whose properties can be read.
    pub(crate) fn of(value: Option<&serde_json::Value>) -> Option<Self> {
        match value {
            None => Some(Self::Undefined),
            Some(serde_json::Value::Null) => Some(Self::Null),
            Some(_) => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Undefined => "undefined",
            Self::Null => "null",
        }
    }
}

impl JsRuntime {
    /// The `TypeError` of reading `property` off a nullish value. JavaScriptCore quotes the source `expression`
    /// of the bundled `sdk.mjs`, V8 names the property.
    pub(crate) fn read_error(self, value: Nullish, property: &str, expression: &str) -> crate::Error {
        crate::Error::TypeError(match self {
            Self::Node => format!("Cannot read properties of {} (reading '{property}')", value.name()),
            Self::Bun => format!("{} is not an object (evaluating '{expression}')", value.name()),
        })
    }

    /// The `TypeError` of destructuring `property` out of a nullish value that an expression produced.
    pub(crate) fn destructure_error(self, value: Nullish, property: &str) -> crate::Error {
        crate::Error::TypeError(match self {
            Self::Node => format!(
                "Cannot destructure property '{property}' of '(intermediate value)' as it is {}.",
                value.name()
            ),
            Self::Bun => format!("Cannot destructure property '{property}' from null or undefined value"),
        })
    }
}
