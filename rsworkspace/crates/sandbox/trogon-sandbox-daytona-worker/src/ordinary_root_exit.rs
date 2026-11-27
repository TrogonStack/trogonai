use serde::Deserialize;

/// How the command's root process ended: by exiting with a code, or by a signal.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct OrdinaryRootExit {
    pub(crate) exit_code: Option<i32>,
    pub(crate) signal: Option<i32>,
}

impl OrdinaryRootExit {
    /// Exactly one of a code in 0..=255 or a signal in 1..=64.
    pub(crate) fn is_valid(self) -> bool {
        (self.exit_code.is_some() ^ self.signal.is_some())
            && self.exit_code.is_none_or(|code| (0..=255).contains(&code))
            && self.signal.is_none_or(|signal| (1..=64).contains(&signal))
    }

    /// The exit code a shell reports: the code itself, or 128 plus the signal.
    pub(crate) fn code(self) -> i32 {
        self.exit_code
            .or(self.signal.map(|signal| 128 + signal))
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests;
