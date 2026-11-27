//! The per-command configuration: each field that is set replaces the
//! session's value for that one command, and each field left out keeps the
//! session's. The exception is `git.safeDirectories`, which adds to the
//! session's.

use std::path::PathBuf;

use trogon_sandbox_contract::{
    DomainPattern, FilesystemEnforcement, FsPathEntry, FsPolicy, FsPolicyError, LiteralPath, NestedUserNamespaceAccess,
    PlatformOptions, PtyAccess,
};

use crate::config::PolicyExecConfig;
use crate::error::RuntimeError;
use crate::settings::{CredentialContext, CredentialOverride, EffectiveCredentials};

/// The filesystem fields of a command's overrides. `allow_read` holds the
/// paths re-allowed within a denied read.
#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct FilesystemOverrides {
    pub deny_read: Option<Vec<FsPathEntry>>,
    pub allow_read: Option<Vec<FsPathEntry>>,
    pub allow_write: Option<Vec<FsPathEntry>>,
    pub deny_write: Option<Vec<FsPathEntry>>,
    pub disabled: Option<bool>,
}

impl FilesystemOverrides {
    pub fn deny_read(mut self, paths: Vec<FsPathEntry>) -> Self {
        self.deny_read = Some(paths);
        self
    }

    pub fn allow_read(mut self, paths: Vec<FsPathEntry>) -> Self {
        self.allow_read = Some(paths);
        self
    }

    pub fn allow_write(mut self, paths: Vec<FsPathEntry>) -> Self {
        self.allow_write = Some(paths);
        self
    }

    pub fn deny_write(mut self, paths: Vec<FsPathEntry>) -> Self {
        self.deny_write = Some(paths);
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = Some(disabled);
        self
    }

    /// `session` with each list that is set here in its place. A list the
    /// policy refuses, such as a deny glob that ends in a separator, is an
    /// error.
    pub fn over(&self, session: &FsPolicy) -> Result<FsPolicy, FsPolicyError> {
        let pick =
            |own: &Option<Vec<FsPathEntry>>, session: &[FsPathEntry]| own.clone().unwrap_or_else(|| session.to_vec());
        FsPolicy::new(
            pick(&self.deny_read, session.deny_read()),
            session.credential_deny_read().to_vec(),
            pick(&self.allow_read, session.allow_read_within_deny()),
            session.own_allow_within_deny().to_vec(),
            pick(&self.allow_write, session.allow_write()),
            pick(&self.deny_write, session.deny_write()),
        )
    }

    /// Whether any allow list is set, which `trogon-sandbox-fence-helper exec` cannot take per
    /// command.
    pub fn has_allows(&self) -> bool {
        self.allow_read.as_ref().is_some_and(|l| !l.is_empty())
            || self.allow_write.as_ref().is_some_and(|l| !l.is_empty())
    }
}

/// The network fields of a command's overrides. The command's
/// `network.allowedDomains` scopes credentials that name no hosts of their
/// own, and decides whether the network is restricted at all; the proxy's own
/// filter stays the session's, so `CommandSpec::network_policy` is what
/// changes it for one command.
#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct NetworkOverrides {
    pub allowed_domains: Option<Vec<DomainPattern>>,
}

impl NetworkOverrides {
    pub fn allowed_domains(mut self, domains: Vec<DomainPattern>) -> Self {
        self.allowed_domains = Some(domains);
        self
    }
}

/// The parts of the configuration one command can change. The fields no
/// command can change (proxy ports, ripgrep, seccomp) have no place here.
#[derive(Debug, Clone, Default)]
pub struct CommandOverrides {
    pub filesystem: Option<FilesystemOverrides>,
    pub network: Option<NetworkOverrides>,
    /// Replaces the session's credentials section as a whole.
    pub credentials: Option<CredentialOverride>,
    /// Whether the command may use pseudo-terminals, read on macOS.
    pub allow_pty: Option<bool>,
    /// Whether the command may create nested user namespaces, read on Linux.
    pub allow_nested_user_namespaces: Option<bool>,
    /// `git.safeDirectories`: added to the session's.
    pub git_safe_directories: Option<Vec<LiteralPath>>,
    /// `network.allowLocalBinding`. Only Windows reads it per command, to
    /// refuse a mode other than the one the session verified.
    pub allow_local_binding: Option<bool>,
    /// `windows.fenceHelper`: the `trogon-sandbox-fence-helper` this command runs with.
    pub fence_helper: Option<PathBuf>,
}

impl CommandOverrides {
    pub fn filesystem(mut self, filesystem: FilesystemOverrides) -> Self {
        self.filesystem = Some(filesystem);
        self
    }

    pub fn network(mut self, network: NetworkOverrides) -> Self {
        self.network = Some(network);
        self
    }

    pub fn credentials(mut self, credentials: CredentialOverride) -> Self {
        self.credentials = Some(credentials);
        self
    }

    pub fn allow_pty(mut self, allow: bool) -> Self {
        self.allow_pty = Some(allow);
        self
    }

    pub fn allow_nested_user_namespaces(mut self, allow: bool) -> Self {
        self.allow_nested_user_namespaces = Some(allow);
        self
    }

    pub fn git_safe_directories(mut self, directories: Vec<LiteralPath>) -> Self {
        self.git_safe_directories = Some(directories);
        self
    }

    pub fn allow_local_binding(mut self, allow: bool) -> Self {
        self.allow_local_binding = Some(allow);
        self
    }

    pub fn fence_helper(mut self, path: impl Into<PathBuf>) -> Self {
        self.fence_helper = Some(path.into());
        self
    }

    /// The rule for `filesystem.disabled`: a command that overrides the
    /// filesystem at all decides it, defaulting to off, so a session-wide
    /// `disabled` does not silently drop a per-command tightening that omits
    /// the key.
    pub fn filesystem_disabled(&self, session: bool) -> bool {
        match &self.filesystem {
            Some(filesystem) => filesystem.disabled.unwrap_or(false),
            None => session,
        }
    }

    /// The session's filesystem policy with this command's lists over it.
    pub fn fs_policy_over(&self, session: &FsPolicy) -> Result<FsPolicy, FsPolicyError> {
        match &self.filesystem {
            Some(filesystem) => filesystem.over(session),
            None => Ok(session.clone()),
        }
    }

    /// The session's platform options with this command's `disabled` and
    /// `git.safeDirectories`: added to the session's.
    pub fn platform_over(&self, session: &PlatformOptions) -> PlatformOptions {
        let mut options = session.clone();
        options.filesystem.disabled =
            FilesystemEnforcement::from(self.filesystem_disabled(session.filesystem.disabled.is_disabled()));
        options.macos.allow_pty = self.allow_pty.map_or(session.macos.allow_pty, PtyAccess::from);
        options.linux.allow_nested_user_namespaces = self.allow_nested_user_namespaces.map_or(
            session.linux.allow_nested_user_namespaces,
            NestedUserNamespaceAccess::from,
        );
        for directory in self.git_safe_directories.iter().flatten() {
            if !options.git_safe_directories.contains(directory) {
                options.git_safe_directories.push(directory.clone());
            }
        }
        options
    }
}

impl CommandOverrides {
    /// The credentials this command runs with, when it names its own section
    /// or its own allowed domains; `None` leaves the session's.
    pub(crate) fn effective_credentials(
        &self,
        config: &PolicyExecConfig,
    ) -> Result<Option<EffectiveCredentials>, RuntimeError> {
        let domains = self
            .network
            .as_ref()
            .and_then(|network| network.allowed_domains.as_deref());
        if self.credentials.is_none() && domains.is_none() {
            return Ok(None);
        }
        let Some(context) = CredentialContext::of(config) else {
            return Err(RuntimeError::Unsupported(
                "a per-command credentials or network.allowedDomains needs a configuration built from settings"
                    .to_string(),
            ));
        };
        context
            .effective(self.credentials.as_ref(), domains)
            .map(Some)
            .map_err(|err| RuntimeError::Invalid(anyhow::anyhow!("{err}")))
    }

    /// `fs` with this command's filesystem lists over it, and with the files
    /// that the command's credentials deny in place of the session's.
    #[cfg_attr(not(unix), allow(dead_code))]
    pub(crate) fn fs_with_credentials(
        &self,
        session: &FsPolicy,
        credential_deny: &[FsPathEntry],
    ) -> Result<FsPolicy, RuntimeError> {
        let own_credentials = self.credentials.is_some();
        let own_deny_read = self
            .filesystem
            .as_ref()
            .is_some_and(|filesystem| filesystem.deny_read.is_some());
        let policy = self
            .fs_policy_over(session)
            .map_err(|err| RuntimeError::Invalid(anyhow::Error::new(err)))?;
        if !own_credentials && !own_deny_read {
            return Ok(policy);
        }
        FsPolicy::new(
            policy.deny_read().to_vec(),
            credential_deny.to_vec(),
            policy.allow_read_within_deny().to_vec(),
            policy.own_allow_within_deny().to_vec(),
            policy.allow_write().to_vec(),
            policy.deny_write().to_vec(),
        )
        .map_err(|err| RuntimeError::Invalid(anyhow::Error::new(err)))
    }
}

#[cfg(test)]
mod tests;
