//! Where the runtime keeps a person's things.

use std::path::{Path, PathBuf};

/// The `.trogonai-agent` directory belonging to one person, and everything inside it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Profile {
    root: PathBuf,
}

impl Profile {
    /// The name a configured provider's saved key is stored under.
    ///
    /// Provider ids hold only letters, digits, `_` and `-`, so the name is
    /// always a single file name and never the gateway key's own.
    #[must_use]
    pub fn provider_api_key_file(provider: &str) -> String {
        format!("{}{provider}", Self::PROVIDER_API_KEY_PREFIX)
    }

    /// The profile directory's own name, relative to a home directory.
    pub const DIR_NAME: &'static str = ".trogonai-agent";

    /// Name of the stored OAuth session, within the profile directory.
    pub const AUTH_FILE: &'static str = "auth.json";
    /// Name of the stored ChatGPT OAuth session, within the profile directory.
    pub const CHATGPT_AUTH_FILE: &'static str = "chatgpt-auth.json";
    /// Name of the stored Grok OAuth session, within the profile directory.
    pub const GROK_AUTH_FILE: &'static str = "grok-auth.json";
    /// Name of the stored API key, within the profile directory.
    pub const API_KEY_FILE: &'static str = "api-key";
    /// What the name of a configured provider's stored key starts with. The
    /// provider id follows it.
    pub const PROVIDER_API_KEY_PREFIX: &'static str = "api-key-";
    /// Name of the directory holding saved sessions.
    pub const SESSIONS_DIR: &'static str = "sessions";
    /// Name of the prompt history log.
    pub const PROMPT_HISTORY_FILE: &'static str = "history.jsonl";
    /// Name of the usage log.
    pub const USAGE_FILE: &'static str = "usage.jsonl";
    /// Name of the directory holding usage records awaiting recovery.
    pub const USAGE_RECOVERY_DIR: &'static str = "usage-recovery";
    /// Name of the directory holding file backups.
    pub const BACKUPS_DIR: &'static str = "backups";
    /// Name of the directory holding MCP server credentials.
    pub const MCP_CREDENTIALS_DIR: &'static str = "mcp-credentials";
    /// Name of the credential store inside [`Self::MCP_CREDENTIALS_DIR`].
    pub const MCP_CREDENTIALS_FILE: &'static str = "credentials.json";

    const SETTINGS_FILE: &'static str = "settings.json";
    const MCP_CONFIG_FILE: &'static str = "mcp.json";
    const MANAGED_SKILLS_DIR: &'static str = "skills";
    const WORKTREES_DIR: &'static str = "worktrees";
    const LOGS_DIR: &'static str = "logs";
    const TRACE_LOG_FILE: &'static str = "trace.log";
    /// Name of the directory holding diagnostic reports.
    pub const DIAGNOSTICS_DIR: &'static str = "diagnostics";
    /// Name of the previous exit's shutdown report, within [`Self::DIAGNOSTICS_DIR`].
    pub const LAST_SHUTDOWN_FILE: &'static str = "last-shutdown.json";
    const RECORDINGS_DIR: &'static str = "recordings";
    const THEMES_DIR: &'static str = "themes";

    /// The profile under a home directory.
    #[must_use]
    pub fn in_home(home: impl AsRef<Path>) -> Self {
        Self {
            root: home.as_ref().join(Self::DIR_NAME),
        }
    }

    /// The profile at an already-known location.
    ///
    /// The caller supplies the selected directory itself. Its name is arbitrary;
    /// this does not append the default profile directory name.
    #[must_use]
    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The profile directory itself.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Settings written by `trogonai-agent config`.
    #[must_use]
    pub fn settings(&self) -> PathBuf {
        self.root.join(Self::SETTINGS_FILE)
    }

    /// The configured MCP servers.
    #[must_use]
    pub fn mcp_config(&self) -> PathBuf {
        self.root.join(Self::MCP_CONFIG_FILE)
    }

    /// Where MCP server credentials are kept.
    #[must_use]
    pub fn mcp_credentials_dir(&self) -> PathBuf {
        self.root.join(Self::MCP_CREDENTIALS_DIR)
    }

    /// The MCP credential store.
    #[must_use]
    pub fn mcp_credentials(&self) -> PathBuf {
        self.mcp_credentials_dir().join(Self::MCP_CREDENTIALS_FILE)
    }

    /// Where skills installed by the runtime are kept.
    #[must_use]
    pub fn managed_skills_dir(&self) -> PathBuf {
        self.root.join(Self::MANAGED_SKILLS_DIR)
    }

    /// Where the git worktrees of isolated child agents are made, one
    /// directory per repository and one below that per child.
    ///
    /// Inside the profile rather than next to the repository, so nothing a
    /// child writes lands in the parent's checkout or in a tool's walk of it.
    #[must_use]
    pub fn worktrees_dir(&self) -> PathBuf {
        self.root.join(Self::WORKTREES_DIR)
    }

    /// Where custom terminal themes are kept.
    #[must_use]
    pub fn themes_dir(&self) -> PathBuf {
        self.root.join(Self::THEMES_DIR)
    }

    /// The stored OAuth session.
    #[must_use]
    pub fn auth(&self) -> PathBuf {
        self.root.join(Self::AUTH_FILE)
    }

    /// The stored ChatGPT OAuth session.
    #[must_use]
    pub fn chatgpt_auth(&self) -> PathBuf {
        self.root.join(Self::CHATGPT_AUTH_FILE)
    }

    /// The stored API key.
    #[must_use]
    pub fn api_key(&self) -> PathBuf {
        self.root.join(Self::API_KEY_FILE)
    }

    /// Where saved sessions are kept.
    #[must_use]
    pub fn sessions_dir(&self) -> PathBuf {
        self.root.join(Self::SESSIONS_DIR)
    }

    /// The prompt history log.
    #[must_use]
    pub fn prompt_history(&self) -> PathBuf {
        self.root.join(Self::PROMPT_HISTORY_FILE)
    }

    /// The usage log.
    ///
    /// The name is still [`Self::USAGE_FILE`] for that caller; this is for the ones that have a
    /// path and no handle.
    #[must_use]
    pub fn usage(&self) -> PathBuf {
        self.root.join(Self::USAGE_FILE)
    }

    /// Where usage records await recovery.
    #[must_use]
    pub fn usage_recovery_dir(&self) -> PathBuf {
        self.root.join(Self::USAGE_RECOVERY_DIR)
    }

    /// Where file backups are kept.
    #[must_use]
    pub fn backups_dir(&self) -> PathBuf {
        self.root.join(Self::BACKUPS_DIR)
    }

    /// Where logs are kept.
    #[must_use]
    pub fn logs_dir(&self) -> PathBuf {
        self.root.join(Self::LOGS_DIR)
    }

    /// The trace log.
    #[must_use]
    pub fn trace_log(&self) -> PathBuf {
        self.logs_dir().join(Self::TRACE_LOG_FILE)
    }

    /// Where process diagnostics are kept.
    #[must_use]
    pub fn diagnostics_dir(&self) -> PathBuf {
        self.root.join(Self::DIAGNOSTICS_DIR)
    }

    /// The last interactive shutdown breakdown, shown by `/trace`.
    #[must_use]
    pub fn last_shutdown_report(&self) -> PathBuf {
        self.diagnostics_dir().join(Self::LAST_SHUTDOWN_FILE)
    }

    /// Where terminal recordings are kept.
    #[must_use]
    pub fn recordings_dir(&self) -> PathBuf {
        self.root.join(Self::RECORDINGS_DIR)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_paths_preserve_current_default_locations() {
        let profile = Profile::in_home("/tmp/fake-home");

        assert_eq!(profile.root(), Path::new("/tmp/fake-home/.trogonai-agent"));
        assert_eq!(
            profile.settings(),
            Path::new("/tmp/fake-home/.trogonai-agent/settings.json")
        );
        assert_eq!(
            profile.mcp_config(),
            Path::new("/tmp/fake-home/.trogonai-agent/mcp.json")
        );
        assert_eq!(
            profile.mcp_credentials_dir(),
            Path::new("/tmp/fake-home/.trogonai-agent/mcp-credentials")
        );
        assert_eq!(
            profile.mcp_credentials(),
            Path::new("/tmp/fake-home/.trogonai-agent/mcp-credentials/credentials.json")
        );
        assert_eq!(
            profile.managed_skills_dir(),
            Path::new("/tmp/fake-home/.trogonai-agent/skills")
        );
        assert_eq!(profile.auth(), Path::new("/tmp/fake-home/.trogonai-agent/auth.json"));
        assert_eq!(
            profile.chatgpt_auth(),
            Path::new("/tmp/fake-home/.trogonai-agent/chatgpt-auth.json")
        );
        assert_eq!(profile.api_key(), Path::new("/tmp/fake-home/.trogonai-agent/api-key"));
        assert_eq!(
            profile.sessions_dir(),
            Path::new("/tmp/fake-home/.trogonai-agent/sessions")
        );
        assert_eq!(
            profile.prompt_history(),
            Path::new("/tmp/fake-home/.trogonai-agent/history.jsonl")
        );
        assert_eq!(
            profile.backups_dir(),
            Path::new("/tmp/fake-home/.trogonai-agent/backups")
        );
        assert_eq!(profile.logs_dir(), Path::new("/tmp/fake-home/.trogonai-agent/logs"));
        assert_eq!(
            profile.trace_log(),
            Path::new("/tmp/fake-home/.trogonai-agent/logs/trace.log")
        );
        assert_eq!(
            profile.diagnostics_dir(),
            Path::new("/tmp/fake-home/.trogonai-agent/diagnostics")
        );
        assert_eq!(
            profile.last_shutdown_report(),
            Path::new("/tmp/fake-home/.trogonai-agent/diagnostics/last-shutdown.json")
        );
        assert_eq!(
            profile.recordings_dir(),
            Path::new("/tmp/fake-home/.trogonai-agent/recordings")
        );
        assert_eq!(profile.usage(), Path::new("/tmp/fake-home/.trogonai-agent/usage.jsonl"));
        assert_eq!(
            profile.usage_recovery_dir(),
            Path::new("/tmp/fake-home/.trogonai-agent/usage-recovery")
        );
    }

    #[test]
    fn every_name_a_caller_opens_by_hand_is_the_last_segment_of_a_path() {
        let profile = Profile::in_home("/home/someone");

        for (whole, name) in [
            (profile.auth(), Profile::AUTH_FILE),
            (profile.chatgpt_auth(), Profile::CHATGPT_AUTH_FILE),
            (profile.root().join(Profile::GROK_AUTH_FILE), Profile::GROK_AUTH_FILE),
            (profile.api_key(), Profile::API_KEY_FILE),
            (profile.sessions_dir(), Profile::SESSIONS_DIR),
            (profile.prompt_history(), Profile::PROMPT_HISTORY_FILE),
            (profile.usage(), Profile::USAGE_FILE),
            (profile.usage_recovery_dir(), Profile::USAGE_RECOVERY_DIR),
            (profile.backups_dir(), Profile::BACKUPS_DIR),
            (profile.mcp_credentials_dir(), Profile::MCP_CREDENTIALS_DIR),
            (profile.mcp_credentials(), Profile::MCP_CREDENTIALS_FILE),
        ] {
            assert_eq!(
                whole.file_name().and_then(|n| n.to_str()),
                Some(name),
                "a caller opening `{name}` relative to a handle must land where the path does",
            );
        }
    }

    #[test]
    fn a_profile_can_be_rooted_where_it_is_rather_than_where_home_is() {
        assert_eq!(
            Profile::at("/var/lib/trogonai-profile").settings(),
            Path::new("/var/lib/trogonai-profile/settings.json")
        );
        assert_eq!(
            Profile::at(Profile::in_home("/home/someone").root().to_path_buf()),
            Profile::in_home("/home/someone"),
        );
    }
}
