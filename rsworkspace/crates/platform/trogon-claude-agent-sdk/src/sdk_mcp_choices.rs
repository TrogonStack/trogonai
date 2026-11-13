//! The choices a tool author makes where the JSON Schema `tools/list` sends cannot tell two zod schemas apart,
//! and the callbacks that stand in for the user code of `.refine` and `.transform`.
//!
//! Nothing here reaches the wire: the choices live beside the tool's JSON Schema, keyed by the JSON Pointer of
//! the schema node they apply to, and only the validation of the TypeScript dialect reads them. What
//! `tools/list` sends stays the schema the author gave.

use crate::JsonObject;
use serde_json::Value;
use std::{fmt, sync::Arc};

/// `z.union` or `z.discriminatedUnion`: both list as `anyOf`. A failure reports every option's issues in an
/// `invalid_union` for the first; the second picks the option by the literal under the discriminator and
/// reports one `invalid_union` without options when none matches.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UnionChoice {
    Plain,
    Discriminated { discriminator: String },
}

/// `z.nullable(schema)` or `z.union([schema, z.null()])`: both list as `anyOf`; a failure reports the inner
/// issues for the first and an `invalid_union` for the second.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NullabilityChoice {
    Nullable,
    UnionWithNull,
}

/// `.length(n)` or `.min(n).max(n)`: both list `minLength`/`maxLength` (`minItems`/`maxItems`) alike; a
/// failure reports one `exact` issue for the first and a bound issue for the second.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LengthChoice {
    Exact,
    MinMax,
}

/// A check of a string schema whose position in the declared chain a choice orders.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StringCheckKind {
    Minimum,
    Maximum,
    /// The `pattern` or `format` check.
    Format,
}

/// `z.int32()` and `z.uint32()` or `z.number().int().min(..).max(..)`: the bounds list alike; a fraction is
/// reported as `int32` or `uint32` for the first and as `safeint` for the second.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NumberFormatChoice {
    Inferred,
    BoundedInteger,
}

/// `z.iso.time()` or `z.string().regex(..)`: both list a bare `pattern`; a failure reports the `time` format
/// for the first and `regex` for the second.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StringFormatChoice {
    PlainPattern,
    IsoTime,
}

/// One choice for the schema node a pointer names. Without one, the node follows the first variant of its
/// kind, which is the mapping zod's own `toJSONSchema` makes most likely.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SchemaChoice {
    Union(UnionChoice),
    Nullability(NullabilityChoice),
    Length(LengthChoice),
    /// The declared order of the checks of a string schema; each kind appears once.
    StringCheckOrder(Vec<StringCheckKind>),
    NumberFormat(NumberFormatChoice),
    StringFormat(StringFormatChoice),
    /// The flags of the `RegExp` behind a `pattern`, which the listed JSON Schema drops.
    RegexFlags(String),
}

impl SchemaChoice {
    pub(crate) fn kind(&self) -> &'static str {
        match self {
            Self::Union(_) => "union",
            Self::Nullability(_) => "nullability",
            Self::Length(_) => "length",
            Self::StringCheckOrder(_) => "string check order",
            Self::NumberFormat(_) => "number format",
            Self::StringFormat(_) => "string format",
            Self::RegexFlags(_) => "regex flags",
        }
    }

    pub(crate) fn annotate(&self, target: &mut JsonObject) {
        match self {
            Self::Union(UnionChoice::Plain) => {}
            Self::Union(UnionChoice::Discriminated { discriminator }) => {
                target.insert("x-trogon-discriminator".into(), discriminator.as_str().into());
            }
            Self::Nullability(NullabilityChoice::Nullable) => {}
            Self::Nullability(NullabilityChoice::UnionWithNull) => {
                target.insert("x-trogon-union-with-null".into(), true.into());
            }
            Self::Length(LengthChoice::Exact) => {}
            Self::Length(LengthChoice::MinMax) => {
                target.insert("x-trogon-min-max".into(), true.into());
            }
            Self::StringCheckOrder(order) => {
                let names = order.iter().map(|kind| match kind {
                    StringCheckKind::Minimum => Value::from("minimum"),
                    StringCheckKind::Maximum => Value::from("maximum"),
                    StringCheckKind::Format => Value::from("format"),
                });
                target.insert("x-trogon-check-order".into(), Value::Array(names.collect()));
            }
            Self::NumberFormat(NumberFormatChoice::Inferred) => {}
            Self::NumberFormat(NumberFormatChoice::BoundedInteger) => {
                target.insert("x-trogon-bounded-integer".into(), true.into());
            }
            Self::StringFormat(StringFormatChoice::PlainPattern) => {}
            Self::StringFormat(StringFormatChoice::IsoTime) => {
                target.insert("x-trogon-iso-time".into(), true.into());
            }
            Self::RegexFlags(flags) => {
                target.insert("x-trogon-flags".into(), flags.as_str().into());
            }
        }
    }
}

/// A failed `.refine` or `.transform`: zod reports it as a `custom` issue at the path of the schema node.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RefineFailure {
    message: String,
}

impl RefineFailure {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    /// The failure of `.refine(check)` without a message.
    pub fn invalid() -> Self {
        Self::new("Invalid input")
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

/// `.refine(check)`: `Err` is a `custom` issue.
pub type Refinement = Arc<dyn Fn(&Value) -> Result<(), RefineFailure> + Send + Sync>;

/// `.transform(map)`: the mapped value is what the handler receives; `Err` is a `custom` issue.
pub type Transformation = Arc<dyn Fn(Value) -> Result<Value, RefineFailure> + Send + Sync>;

/// The user code of a schema node: the refinement runs after the node's own checks (unless one aborted them),
/// then the transformation runs when no check failed.
#[derive(Clone, Default)]
pub struct SchemaCallbackOptions {
    pub refine: Option<Refinement>,
    pub transform: Option<Transformation>,
}

#[derive(Clone)]
pub struct SchemaCallback {
    refine: Option<Refinement>,
    transform: Option<Transformation>,
}

impl From<SchemaCallbackOptions> for SchemaCallback {
    fn from(options: SchemaCallbackOptions) -> Self {
        Self {
            refine: options.refine,
            transform: options.transform,
        }
    }
}

impl SchemaCallback {
    pub(crate) fn refine(&self, value: &Value) -> Result<(), RefineFailure> {
        self.refine.as_ref().map_or(Ok(()), |refine| refine(value))
    }

    pub(crate) fn transform(&self, value: Value) -> Result<Value, RefineFailure> {
        match &self.transform {
            Some(transform) => transform(value),
            None => Ok(value),
        }
    }
}

impl fmt::Debug for SchemaCallback {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SchemaCallback")
            .field("refine", &self.refine.is_some())
            .field("transform", &self.transform.is_some())
            .finish()
    }
}

#[cfg(test)]
#[path = "sdk_mcp_choices_tests.rs"]
mod tests;
