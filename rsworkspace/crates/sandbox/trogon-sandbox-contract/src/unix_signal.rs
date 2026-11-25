use serde::{Deserialize, Serialize};

/// A Unix signal a caller can send to a previously started command.
///
/// It is a closed enum rather than a free-form signal name, so an arbitrary string can never reach a worker's
/// `kill -<SIGNAL>` invocation. It serializes as the snake case variant name, and `Term` is the default.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum UnixSignal {
    #[default]
    Term,
    Kill,
    Int,
    Hup,
    Quit,
    Usr1,
    Usr2,
}

impl UnixSignal {
    /// The signal name without the `SIG` prefix, as `kill -<NAME>` takes it.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Term => "TERM",
            Self::Kill => "KILL",
            Self::Int => "INT",
            Self::Hup => "HUP",
            Self::Quit => "QUIT",
            Self::Usr1 => "USR1",
            Self::Usr2 => "USR2",
        }
    }
}
