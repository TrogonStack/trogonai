use std::fmt;

use cel_interpreter::ExecutionError;

/// The class of failure a rule hit while running.
///
/// Only the class is kept. The interpreter's own messages quote the values
/// involved, which are request data, and a refusal is logged and audited.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeErrorKind {
    /// The rule named a variable or function that does not exist.
    UndeclaredReference,
    /// The rule read a field the facts do not have.
    MissingField,
    /// An operator or function was given a value of the wrong type.
    TypeMismatch,
    /// Division or remainder by zero, or an integer overflow.
    Arithmetic,
    /// A function rejected its arguments.
    Function,
}

impl fmt::Display for RuntimeErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::UndeclaredReference => "the rule references a name that is not defined",
            Self::MissingField => "the rule reads a field the facts do not have",
            Self::TypeMismatch => "the rule applied an operation to a value of the wrong type",
            Self::Arithmetic => "the rule failed in arithmetic",
            Self::Function => "a function rejected its arguments",
        })
    }
}

impl RuntimeErrorKind {
    pub(crate) fn of(error: &ExecutionError) -> Self {
        match error {
            ExecutionError::UndeclaredReference(_) => Self::UndeclaredReference,
            ExecutionError::NoSuchKey(_) => Self::MissingField,
            ExecutionError::DivisionByZero(_) | ExecutionError::RemainderByZero(_) | ExecutionError::Overflow(..) => {
                Self::Arithmetic
            }
            ExecutionError::FunctionError { .. }
            | ExecutionError::InvalidArgumentCount { .. }
            | ExecutionError::MissingArgumentOrTarget => Self::Function,
            _ => Self::TypeMismatch,
        }
    }
}
