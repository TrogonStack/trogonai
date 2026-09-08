//! An `rpc` method's name exactly as the protobuf descriptor spelled it
//! (ADR 0016 §2).

/// Untrusted method name text. Carries no guarantee that the value is a legal
/// protobuf identifier, a legal subject token, or a legal micro endpoint name;
/// [`crate::MethodName::from_input`] is the single conversion into the domain
/// value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MethodNameInput(Box<str>);

impl MethodNameInput {
    pub fn new(value: impl Into<Box<str>>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}
