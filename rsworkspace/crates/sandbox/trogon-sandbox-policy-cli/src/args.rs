use std::fmt;
use std::path::PathBuf;

use crate::shell;

#[cfg(unix)]
use std::os::fd::RawFd;
#[cfg(windows)]
type RawFd = i32;

#[derive(Debug, Default, Eq, PartialEq)]
pub struct Args {
    pub debug: bool,
    pub settings: Option<PathBuf>,
    pub command_string: Option<String>,
    pub guest_view: bool,
    pub control_fd: Option<ControlFd>,
    pub command: Vec<String>,
}

/// The descriptor settings updates arrive on: never stdin, stdout or stderr.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct ControlFd(RawFd);

impl ControlFd {
    const FIRST: RawFd = 3;

    pub fn parse(text: &str) -> Result<Self, ControlFdError> {
        let invalid = || ControlFdError(text.to_string());
        if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
            return Err(invalid());
        }
        let fd: RawFd = text.parse().map_err(|_| invalid())?;
        if fd < Self::FIRST {
            return Err(invalid());
        }
        Ok(Self(fd))
    }

    #[cfg_attr(windows, allow(dead_code))]
    pub fn raw(self) -> RawFd {
        self.0
    }
}

impl fmt::Display for ControlFd {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct ControlFdError(String);

impl fmt::Display for ControlFdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "--control-fd must be an integer file descriptor >= 3 (got {:?})",
            self.0
        )
    }
}

/// What to run, and how it was asked for.
#[derive(Debug, Eq, PartialEq)]
pub enum CommandLine {
    /// `-c`: passed to the shell as written.
    ShellString(String),
    /// Positional arguments: each stays one word.
    Words(Vec<String>),
}

impl CommandLine {
    pub fn from_args(args: &Args) -> Option<Self> {
        match (
            args.command_string.as_ref().filter(|text| !text.is_empty()),
            args.command.is_empty(),
        ) {
            (Some(text), _) => Some(Self::ShellString(text.clone())),
            (None, false) => Some(Self::Words(args.command.clone())),
            (None, true) => None,
        }
    }

    pub fn shell_text(&self) -> String {
        match self {
            Self::ShellString(text) => text.clone(),
            Self::Words(words) => shell::join(words),
        }
    }

    #[cfg_attr(windows, allow(dead_code))]
    pub fn debug_line(&self) -> String {
        match self {
            Self::ShellString(text) => format!("Command string mode: {text}"),
            Self::Words(_) => format!("Original command: {}", self.shell_text()),
        }
    }
}

#[cfg(test)]
mod tests;
