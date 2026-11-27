//! A typed model of the Seatbelt profile language (SBPL).
//!
//! A profile is a list of allow rules followed by a list of deny rules, after
//! a `(deny default)`. Later rules win over earlier ones, so a deny can only
//! be trusted when nothing that grants access comes after it. The builder
//! enforces that with its type: [`ProfileBuilder<AllowPhase>`] accepts allow
//! rules, [`ProfileBuilder::begin_denies`] consumes it, and only the result
//! accepts deny rules. There is no way to add an allow once a deny exists.
//!
//! ```compile_fail
//! use trogon_sandbox_subprocess_worker::isolation::seatbelt::sbpl::*;
//!
//! fn late_allow() -> Result<(), SbplError> {
//!     let denies = ProfileBuilder::new(LogTag::new("TAG")?).begin_denies();
//!     let _ = denies.allow(AllowRule::new([Operation::FileReadStar], []));
//!     Ok(())
//! }
//! ```

use std::fmt::{self, Write};
use std::marker::PhantomData;

#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
pub enum SbplError {
    #[error("a profile string of {len} bytes exceeds the {max} byte limit")]
    StringTooLong { len: usize, max: usize },
    #[error("a profile string cannot hold the control character {0:?}")]
    ControlCharacter(char),
    #[error("the profile is {len} bytes, more than the {max} byte limit")]
    ProfileTooLarge { len: usize, max: usize },
    #[error("the glob set [{from:?}-{to:?}] is too wide to express as a profile regex")]
    SetTooWide { from: char, to: char },
    #[error("the glob {0:?} has a set that spans path segments, which a profile cannot express")]
    UnsupportedGlob(String),
}

/// The contents of a profile string literal.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct SbplString(String);

impl SbplString {
    pub fn new(raw: impl Into<String>) -> Result<Self, SbplError> {
        let raw = raw.into();
        if raw.len() > MAX_STRING_BYTES {
            return Err(SbplError::StringTooLong {
                len: raw.len(),
                max: MAX_STRING_BYTES,
            });
        }
        if let Some(control) = raw.chars().find(|c| c.is_control() && !matches!(c, '\n' | '\t' | '\r')) {
            return Err(SbplError::ControlCharacter(control));
        }
        Ok(Self(raw))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SbplString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_char('"')?;
        for c in self.0.chars() {
            match c {
                '\\' => f.write_str("\\\\")?,
                '"' => f.write_str("\\\"")?,
                '\n' => f.write_str("\\n")?,
                '\t' => f.write_str("\\t")?,
                '\r' => f.write_str("\\r")?,
                other => f.write_char(other)?,
            }
        }
        f.write_char('"')
    }
}

/// The marker the kernel appends to every denial of one profile, so a log
/// stream can be narrowed to one launch.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct LogTag(SbplString);

impl LogTag {
    pub fn new(tag: impl Into<String>) -> Result<Self, SbplError> {
        SbplString::new(tag).map(Self)
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

/// Escapes the characters a profile regex treats as operators.
pub fn escape_regex(literal: &str) -> String {
    let mut escaped = String::with_capacity(literal.len());
    for c in literal.chars() {
        if matches!(
            c,
            '.' | '*' | '+' | '?' | '^' | '$' | '{' | '}' | '(' | ')' | '|' | '[' | ']' | '\\'
        ) {
            escaped.push('\\');
        }
        escaped.push(c);
    }
    escaped
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum VnodeType {
    Directory,
    CharacterDevice,
}

impl VnodeType {
    fn keyword(self) -> &'static str {
        match self {
            Self::Directory => "DIRECTORY",
            Self::CharacterDevice => "CHARACTER-DEVICE",
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum Filter {
    Literal(SbplString),
    Subpath(SbplString),
    Regex(SbplString),
    GlobalName(SbplString),
    GlobalNamePrefix(SbplString),
    SysctlName(SbplString),
    SysctlNamePrefix(SbplString),
    IokitRegistryEntryClass(SbplString),
    IokitUserClientClass(SbplString),
    RemoteIp(SbplString),
    LocalIp(SbplString),
    VnodeType(VnodeType),
    SocketDomainSystem,
    SocketDomainUnix,
    SocketProtocol(u8),
    LocalUnixSocket(Box<Filter>),
    RemoteUnixSocket(Box<Filter>),
    PathRegex(SbplString),
    TargetSameSandbox,
    RequireAll(Vec<Filter>),
    RequireAny(Vec<Filter>),
    RequireNot(Box<Filter>),
}

impl Filter {
    pub fn literal(path: &str) -> Result<Self, SbplError> {
        SbplString::new(path).map(Self::Literal)
    }

    pub fn subpath(path: &str) -> Result<Self, SbplError> {
        SbplString::new(path).map(Self::Subpath)
    }

    pub fn regex(source: &str) -> Result<Self, SbplError> {
        SbplString::new(source).map(Self::Regex)
    }

    pub fn global_name(name: &str) -> Result<Self, SbplError> {
        SbplString::new(name).map(Self::GlobalName)
    }

    pub fn global_name_prefix(name: &str) -> Result<Self, SbplError> {
        SbplString::new(name).map(Self::GlobalNamePrefix)
    }

    pub fn sysctl_name(name: &str) -> Result<Self, SbplError> {
        SbplString::new(name).map(Self::SysctlName)
    }

    pub fn sysctl_name_prefix(name: &str) -> Result<Self, SbplError> {
        SbplString::new(name).map(Self::SysctlNamePrefix)
    }

    pub fn iokit_registry_entry_class(name: &str) -> Result<Self, SbplError> {
        SbplString::new(name).map(Self::IokitRegistryEntryClass)
    }

    pub fn iokit_user_client_class(name: &str) -> Result<Self, SbplError> {
        SbplString::new(name).map(Self::IokitUserClientClass)
    }

    pub fn remote_ip(endpoint: &str) -> Result<Self, SbplError> {
        SbplString::new(endpoint).map(Self::RemoteIp)
    }

    pub fn local_ip(endpoint: &str) -> Result<Self, SbplError> {
        SbplString::new(endpoint).map(Self::LocalIp)
    }

    pub fn path_regex(source: &str) -> Result<Self, SbplError> {
        SbplString::new(source).map(Self::PathRegex)
    }

    pub fn local_unix_socket(path: Filter) -> Self {
        Self::LocalUnixSocket(Box::new(path))
    }

    pub fn remote_unix_socket(path: Filter) -> Self {
        Self::RemoteUnixSocket(Box::new(path))
    }

    pub fn excluding(filter: Filter) -> Self {
        Self::RequireNot(Box::new(filter))
    }
}

impl fmt::Display for Filter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Literal(s) => write!(f, "(literal {s})"),
            Self::Subpath(s) => write!(f, "(subpath {s})"),
            Self::Regex(s) => write!(f, "(regex {s})"),
            Self::GlobalName(s) => write!(f, "(global-name {s})"),
            Self::GlobalNamePrefix(s) => write!(f, "(global-name-prefix {s})"),
            Self::SysctlName(s) => write!(f, "(sysctl-name {s})"),
            Self::SysctlNamePrefix(s) => write!(f, "(sysctl-name-prefix {s})"),
            Self::IokitRegistryEntryClass(s) => write!(f, "(iokit-registry-entry-class {s})"),
            Self::IokitUserClientClass(s) => write!(f, "(iokit-user-client-class {s})"),
            Self::RemoteIp(s) => write!(f, "(remote ip {s})"),
            Self::LocalIp(s) => write!(f, "(local ip {s})"),
            Self::VnodeType(kind) => write!(f, "(vnode-type {})", kind.keyword()),
            Self::SocketDomainSystem => f.write_str("(socket-domain AF_SYSTEM)"),
            Self::SocketDomainUnix => f.write_str("(socket-domain AF_UNIX)"),
            Self::LocalUnixSocket(path) => write!(f, "(local unix-socket {path})"),
            Self::RemoteUnixSocket(path) => write!(f, "(remote unix-socket {path})"),
            Self::PathRegex(s) => write!(f, "(path-regex {s})"),
            Self::SocketProtocol(protocol) => write!(f, "(socket-protocol {protocol})"),
            Self::TargetSameSandbox => f.write_str("(target same-sandbox)"),
            Self::RequireAll(filters) => write_combinator(f, "require-all", filters),
            Self::RequireAny(filters) => write_combinator(f, "require-any", filters),
            Self::RequireNot(filter) => write!(f, "(require-not {filter})"),
        }
    }
}

fn write_combinator(f: &mut fmt::Formatter<'_>, name: &str, filters: &[Filter]) -> fmt::Result {
    write!(f, "({name}")?;
    for filter in filters {
        write!(f, " {filter}")?;
    }
    f.write_char(')')
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum Operation {
    ProcessExec,
    ProcessFork,
    ProcessInfoStar,
    Signal,
    MachPrivTaskPort,
    UserPreferenceRead,
    MachLookup,
    IpcPosixShm,
    IpcPosixSem,
    IokitOpen,
    IokitGetProperties,
    SystemSocket,
    SysctlRead,
    SysctlWrite,
    DistributedNotificationPost,
    FileIoctl,
    FileReadStar,
    FileReadData,
    FileReadXattr,
    FileIssueExtension,
    FileWriteStar,
    FileWriteData,
    FileWriteCreate,
    FileWriteUnlink,
    NetworkStar,
    NetworkOutbound,
    NetworkBind,
    NetworkInbound,
    PseudoTty,
    AppleeventSend,
    Lsopen,
}

impl Operation {
    pub fn name(self) -> &'static str {
        match self {
            Self::ProcessExec => "process-exec",
            Self::ProcessFork => "process-fork",
            Self::ProcessInfoStar => "process-info*",
            Self::Signal => "signal",
            Self::MachPrivTaskPort => "mach-priv-task-port",
            Self::UserPreferenceRead => "user-preference-read",
            Self::MachLookup => "mach-lookup",
            Self::IpcPosixShm => "ipc-posix-shm",
            Self::IpcPosixSem => "ipc-posix-sem",
            Self::IokitOpen => "iokit-open",
            Self::IokitGetProperties => "iokit-get-properties",
            Self::SystemSocket => "system-socket",
            Self::SysctlRead => "sysctl-read",
            Self::SysctlWrite => "sysctl-write",
            Self::DistributedNotificationPost => "distributed-notification-post",
            Self::FileIoctl => "file-ioctl",
            Self::FileReadStar => "file-read*",
            Self::FileReadData => "file-read-data",
            Self::FileReadXattr => "file-read-xattr",
            Self::FileIssueExtension => "file-issue-extension",
            Self::FileWriteStar => "file-write*",
            Self::FileWriteData => "file-write-data",
            Self::FileWriteCreate => "file-write-create",
            Self::FileWriteUnlink => "file-write-unlink",
            Self::NetworkStar => "network*",
            Self::NetworkOutbound => "network-outbound",
            Self::NetworkBind => "network-bind",
            Self::NetworkInbound => "network-inbound",
            Self::PseudoTty => "pseudo-tty",
            Self::AppleeventSend => "appleevent-send",
            Self::Lsopen => "lsopen",
        }
    }
}

fn write_rule(out: &mut String, verb: &str, operations: &[Operation], filters: &[Filter], message: Option<&LogTag>) {
    let _ = write!(out, "({verb}");
    for operation in operations {
        let _ = write!(out, " {}", operation.name());
    }
    for filter in filters {
        let _ = write!(out, " {filter}");
    }
    if let Some(tag) = message {
        let _ = write!(out, " (with message {})", tag.0);
    }
    out.push_str(")\n");
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct AllowRule {
    operations: Vec<Operation>,
    filters: Vec<Filter>,
}

impl AllowRule {
    pub fn new(operations: impl IntoIterator<Item = Operation>, filters: impl IntoIterator<Item = Filter>) -> Self {
        Self {
            operations: operations.into_iter().collect(),
            filters: filters.into_iter().collect(),
        }
    }
}

impl fmt::Display for AllowRule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut out = String::new();
        write_rule(&mut out, "allow", &self.operations, &self.filters, None);
        f.write_str(out.trim_end())
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct DenyRule {
    operations: Vec<Operation>,
    filters: Vec<Filter>,
}

impl DenyRule {
    pub fn new(operations: impl IntoIterator<Item = Operation>, filters: impl IntoIterator<Item = Filter>) -> Self {
        Self {
            operations: operations.into_iter().collect(),
            filters: filters.into_iter().collect(),
        }
    }
}

#[derive(Debug)]
pub struct AllowPhase;
#[derive(Debug)]
pub struct DenyPhase;

#[derive(Debug)]
pub struct ProfileBuilder<Phase> {
    tag: LogTag,
    allows: Vec<AllowRule>,
    denies: Vec<DenyRule>,
    phase: PhantomData<Phase>,
}

impl ProfileBuilder<AllowPhase> {
    pub fn new(tag: LogTag) -> Self {
        Self {
            tag,
            allows: Vec::new(),
            denies: Vec::new(),
            phase: PhantomData,
        }
    }

    pub fn allow(mut self, rule: AllowRule) -> Self {
        self.allows.push(rule);
        self
    }

    pub fn begin_denies(self) -> ProfileBuilder<DenyPhase> {
        ProfileBuilder {
            tag: self.tag,
            allows: self.allows,
            denies: self.denies,
            phase: PhantomData,
        }
    }
}

impl ProfileBuilder<DenyPhase> {
    pub fn deny(mut self, rule: DenyRule) -> Self {
        self.denies.push(rule);
        self
    }

    pub fn build(self) -> Result<Profile, SbplError> {
        let mut text = String::new();
        text.push_str("(version 1)\n");
        let _ = writeln!(text, "(deny default (with message {}))", self.tag.0);
        for rule in &self.allows {
            write_rule(&mut text, "allow", &rule.operations, &rule.filters, None);
        }
        for rule in &self.denies {
            write_rule(&mut text, "deny", &rule.operations, &rule.filters, Some(&self.tag));
        }
        if text.len() > MAX_PROFILE_BYTES {
            return Err(SbplError::ProfileTooLarge {
                len: text.len(),
                max: MAX_PROFILE_BYTES,
            });
        }
        Ok(Profile { text, tag: self.tag })
    }
}

#[derive(Debug, Clone)]
pub struct Profile {
    text: String,
    tag: LogTag,
}

impl Profile {
    pub fn as_str(&self) -> &str {
        &self.text
    }

    pub fn tag(&self) -> &LogTag {
        &self.tag
    }
}

#[cfg(test)]
mod tests;
use super::constants::{MAX_PROFILE_BYTES, MAX_STRING_BYTES};
