use trogon_sandbox_contract::{FsPathEntry, FsPolicy, LiteralPath};

use crate::{CompiledFsEntry, FsAccess, LiteralPathExt};

/// Answers what an [`FsPolicy`] lets a sandbox read and write.
///
/// Precedence, applied to one path at a time. An entry covers a path when it names the path or a directory above
/// it, or when a pattern matches the path or a directory above it.
///
/// - Reads are open unless `deny_read` covers the path. A covered path is readable again when an
///   `allow_read_within_deny` entry that can re-open that deny covers it. A literal deny that lies strictly inside
///   a literal allow stays denied: nothing re-opens it. A glob deny is re-opened only by the allow entries the glob
///   itself covers, so a deny of `/**/.env` with an allow of `/work` keeps every `.env` unreadable. Any other
///   literal deny is re-opened by every allow that covers the path. An allow that lies outside every deny changes
///   nothing.
/// - Writes are closed unless `allow_write` covers the path. A covered path is still refused when `deny_write`
///   covers it: the deny wins.
/// - A `credential_deny_read` entry is a read deny that no `allow_read_within_deny` entry can re-open. Only an
///   `own_allow_within_deny` entry can.
/// - Reading and writing are independent.
#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct FsPolicyEvaluator {
    deny_read: Vec<CompiledFsEntry>,
    credential_deny_read: Vec<CompiledFsEntry>,
    allow_read_within_deny: Vec<CompiledFsEntry>,
    own_allow_within_deny: Vec<CompiledFsEntry>,
    allow_write: Vec<CompiledFsEntry>,
    deny_write: Vec<CompiledFsEntry>,
}

fn compile(entries: &[FsPathEntry]) -> Vec<CompiledFsEntry> {
    entries.iter().map(CompiledFsEntry::new).collect()
}

fn any_covers(entries: &[CompiledFsEntry], path: &LiteralPath) -> bool {
    entries.iter().any(|entry| entry.covers(path))
}

impl FsPolicyEvaluator {
    /// Compiles every entry of `policy`.
    pub fn new(policy: &FsPolicy) -> Self {
        Self {
            deny_read: compile(policy.deny_read()),
            credential_deny_read: compile(policy.credential_deny_read()),
            allow_read_within_deny: compile(policy.allow_read_within_deny()),
            own_allow_within_deny: compile(policy.own_allow_within_deny()),
            allow_write: compile(policy.allow_write()),
            deny_write: compile(policy.deny_write()),
        }
    }

    /// The positions in `allow_read_within_deny` of the entries that can re-open `deny`, a `deny_read` entry.
    pub fn read_exceptions(&self, deny: &FsPathEntry) -> Vec<usize> {
        exceptions_among(&self.allow_read_within_deny, &CompiledFsEntry::new(deny))
    }

    /// The positions in `own_allow_within_deny` of the entries that can re-open `deny`, a `credential_deny_read`
    /// entry.
    pub fn credential_read_exceptions(&self, deny: &FsPathEntry) -> Vec<usize> {
        exceptions_among(&self.own_allow_within_deny, &CompiledFsEntry::new(deny))
    }

    /// Whether the sandbox may read `path`.
    pub fn read_access(&self, path: &LiteralPath) -> FsAccess {
        let credential_lifted = self
            .credential_deny_read
            .iter()
            .filter(|deny| deny.covers(path))
            .all(|deny| any_exception_covers(&self.own_allow_within_deny, deny, path));
        if !credential_lifted {
            return FsAccess::Denied;
        }
        let denied = self
            .deny_read
            .iter()
            .any(|deny| deny.covers(path) && !any_exception_covers(&self.allow_read_within_deny, deny, path));
        if denied { FsAccess::Denied } else { FsAccess::Allowed }
    }

    /// Whether the sandbox may write `path`.
    pub fn write_access(&self, path: &LiteralPath) -> FsAccess {
        if any_covers(&self.allow_write, path) && !any_covers(&self.deny_write, path) {
            FsAccess::Allowed
        } else {
            FsAccess::Denied
        }
    }
}

fn any_exception_covers(allows: &[CompiledFsEntry], deny: &CompiledFsEntry, path: &LiteralPath) -> bool {
    exceptions_among(allows, deny)
        .into_iter()
        .any(|index| allows.get(index).is_some_and(|allow| allow.covers(path)))
}

fn exceptions_among(allows: &[CompiledFsEntry], deny: &CompiledFsEntry) -> Vec<usize> {
    match deny {
        CompiledFsEntry::Literal(path) => {
            let nested_in_allowed_literal = allows
                .iter()
                .any(|allow| matches!(allow, CompiledFsEntry::Literal(dir) if path != dir && path.is_within(dir)));
            if nested_in_allowed_literal {
                Vec::new()
            } else {
                (0..allows.len()).collect()
            }
        }
        CompiledFsEntry::Glob { matcher, .. } => allows
            .iter()
            .enumerate()
            .filter(|(_, allow)| allow.sample_path().is_some_and(|sample| matcher.covers(sample)))
            .map(|(index, _)| index)
            .collect(),
    }
}
