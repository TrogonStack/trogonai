//! The error every fallible operation of the helper returns: a system or
//! storage failure, or a message, wrapped in the context of what was being
//! attempted.

use std::fmt;

use crate::logon::{HolderGone, MappedDriveCwd, SeclogonDisabled};
use crate::state_db::ProfileBusy;
use crate::wfp::WfpAccessDenied;

pub type Result<T, E = FenceHelperError> = std::result::Result<T, E>;

#[derive(thiserror::Error)]
pub enum FenceHelperError {
    /// A refusal or failure this code reports in its own words.
    Message(String),
    /// What was being attempted when `source` happened.
    Context {
        context: String,
        #[source]
        source: Box<FenceHelperError>,
    },
    /// A call into the system failed.
    System(#[from] windows::core::Error),
    /// A file or pipe operation failed.
    Io(#[from] std::io::Error),
    /// The state store refused a statement.
    Sqlite(#[from] rusqlite::Error),
    /// A document could not be read or written.
    Json(#[from] serde_json::Error),
    /// The filtering platform refused an admin-gated call.
    WfpAccessDenied(#[from] WfpAccessDenied),
    /// A profile grant or forget is in progress.
    ProfileBusy(#[from] ProfileBusy),
    /// The session that held a launch ended before the command did.
    HolderGone(#[from] HolderGone),
    /// The working directory is on a mapped or network drive.
    MappedDriveCwd(#[from] MappedDriveCwd),
    /// The Secondary Logon service cannot launch the command.
    SeclogonDisabled(#[from] SeclogonDisabled),
    /// An address that does not parse.
    AddrParse(#[from] std::net::AddrParseError),
    /// Text that is not valid UTF-16.
    Utf16(#[from] std::string::FromUtf16Error),
    /// Text that is not valid UTF-8.
    Utf8(#[from] std::string::FromUtf8Error),
    /// A number that does not parse.
    ParseInt(#[from] std::num::ParseIntError),
}

impl FenceHelperError {
    pub fn msg(message: impl Into<String>) -> Self {
        Self::Message(message.into())
    }

    /// This error, wrapped in what was being attempted.
    pub fn context(self, context: impl fmt::Display) -> Self {
        Self::Context {
            context: context.to_string(),
            source: Box::new(self),
        }
    }

    /// The innermost error of the chain of contexts.
    pub fn root(&self) -> &Self {
        match self {
            Self::Context { source, .. } => source.root(),
            other => other,
        }
    }

    /// The system error this one is, or was caused by.
    pub fn system_error(&self) -> Option<&windows::core::Error> {
        match self.root() {
            Self::System(error) => Some(error),
            _ => None,
        }
    }

    /// Whether the filtering platform refused an admin-gated call.
    pub fn is_wfp_access_denied(&self) -> bool {
        matches!(self.root(), Self::WfpAccessDenied(_))
    }

    /// The I/O error this one is, or was caused by.
    pub fn io_error(&self) -> Option<&std::io::Error> {
        match self.root() {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }

    /// Whether a profile grant or forget kept this call out.
    pub fn is_profile_busy(&self) -> bool {
        matches!(self.root(), Self::ProfileBusy(_))
    }

    /// Whether the session that held a launch ended first.
    pub fn is_holder_gone(&self) -> bool {
        matches!(self.root(), Self::HolderGone(_))
    }

    /// The mapped-drive refusal, if that is the cause.
    pub fn mapped_drive_cwd(&self) -> Option<&MappedDriveCwd> {
        match self.root() {
            Self::MappedDriveCwd(error) => Some(error),
            _ => None,
        }
    }

    /// The Secondary Logon refusal, if that is the cause.
    pub fn seclogon_disabled(&self) -> Option<&SeclogonDisabled> {
        match self.root() {
            Self::SeclogonDisabled(error) => Some(error),
            _ => None,
        }
    }
}

/// `{:#}` and `{:?}` print every context down to the root cause, separated
/// by `: `; plain `{}` prints the outermost.
impl fmt::Debug for FenceHelperError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:#}")
    }
}

impl fmt::Display for FenceHelperError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Message(message) => f.write_str(message),
            Self::Context { context, source } => {
                f.write_str(context)?;
                if f.alternate() {
                    write!(f, ": {source:#}")?;
                }
                Ok(())
            }
            Self::System(error) => fmt::Display::fmt(error, f),
            Self::Io(error) => fmt::Display::fmt(error, f),
            Self::Sqlite(error) => fmt::Display::fmt(error, f),
            Self::Json(error) => fmt::Display::fmt(error, f),
            Self::WfpAccessDenied(error) => fmt::Display::fmt(error, f),
            Self::ProfileBusy(error) => fmt::Display::fmt(error, f),
            Self::HolderGone(error) => fmt::Display::fmt(error, f),
            Self::MappedDriveCwd(error) => fmt::Display::fmt(error, f),
            Self::SeclogonDisabled(error) => fmt::Display::fmt(error, f),
            Self::AddrParse(error) => fmt::Display::fmt(error, f),
            Self::Utf16(error) => fmt::Display::fmt(error, f),
            Self::Utf8(error) => fmt::Display::fmt(error, f),
            Self::ParseInt(error) => fmt::Display::fmt(error, f),
        }
    }
}
