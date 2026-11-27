use std::ffi::OsString;

use tokio::process::Command;

/// The program, with any leading arguments, that runs git.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GitCommand {
    program: OsString,
    leading_args: Vec<OsString>,
}

impl GitCommand {
    pub(crate) fn system() -> Self {
        Self::new("git", [])
    }

    pub(crate) fn new(program: impl Into<OsString>, leading_args: impl IntoIterator<Item = OsString>) -> Self {
        Self {
            program: program.into(),
            leading_args: leading_args.into_iter().collect(),
        }
    }

    pub(crate) fn command(&self) -> Command {
        let mut command = Command::new(&self.program);
        command.args(&self.leading_args);
        command
    }
}
