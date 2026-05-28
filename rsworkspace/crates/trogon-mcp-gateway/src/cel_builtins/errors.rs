use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CelBuiltinsError {
    NotImplemented(&'static str),
    WrongArity {
        name: &'static str,
        expected: usize,
        got: usize,
    },
    WrongType {
        name: &'static str,
        position: usize,
        expected: &'static str,
    },
}

impl fmt::Display for CelBuiltinsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotImplemented(name) => write!(f, "cel builtin not implemented: {name}"),
            Self::WrongArity { name, expected, got } => {
                write!(f, "{name}: expected {expected} argument(s), got {got}")
            }
            Self::WrongType {
                name,
                position,
                expected,
            } => write!(
                f,
                "{name}: argument {position} has wrong type (expected {expected})"
            ),
        }
    }
}

impl std::error::Error for CelBuiltinsError {}
