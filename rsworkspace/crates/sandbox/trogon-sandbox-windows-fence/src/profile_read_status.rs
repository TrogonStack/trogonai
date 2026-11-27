use crate::profile_exclusions::ProfileExclusions;
use crate::profile_grant_counts::ProfileGrantCounts;
use crate::profile_grant_item::ProfileGrantItem;
use crate::profile_grant_path::ProfileGrantPath;
use crate::profile_read_state::ProfileReadState;
use crate::profile_read_var::ProfileReadVar;
use crate::windows_sandbox_error::WindowsSandboxError;
use crate::windows_sandbox_error_code::WindowsSandboxErrorCode as Code;

/// The recorded state of the profile read grant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileReadStatus {
    pub state: ProfileReadState,
    pub reason: Option<String>,
    pub exclusions: ProfileExclusions,
    /// The profile directory the grant was made for.
    pub profile: Option<String>,
    /// What `fence-helper` says it does, by name; empty from one that does not say.
    pub capabilities: Vec<String>,
    /// What the last run of the grant left as it found it: objects in the
    /// profile this user may not change.
    pub not_granted: Vec<ProfileGrantItem>,
    pub self_closed: Vec<ProfileGrantItem>,
    pub unverifiable: Vec<ProfileGrantItem>,
    pub unprotected: Vec<ProfileGrantItem>,
    pub stale_allow: Vec<ProfileGrantItem>,
    /// Configuration files granted by name that were replaced since (a save
    /// by write-and-rename): unreadable to sandboxed commands, and their
    /// variables are not in `env`, until the grant runs again.
    pub replaced_files: Vec<String>,
    /// Exclusions added to the built-in list when the grant was made,
    /// relative to the profile.
    pub extra_exclusions: Vec<String>,
    pub counts: ProfileGrantCounts,
    /// Configuration-file variables for a sandboxed command; empty unless
    /// `current`.
    pub env: std::collections::BTreeMap<ProfileReadVar, String>,
    /// Every recorded path; only when asked for.
    pub paths: Option<Vec<ProfileGrantPath>>,
}

impl ProfileReadStatus {
    /// The variables an embedder may set so a
    /// tool reads the real user's files in place. None unless the grant is
    /// current, and never one `supplied` already names (compared without
    /// regard to case). The library sets none of them itself.
    pub fn read_env<'a>(&self, supplied: impl IntoIterator<Item = &'a str>) -> Vec<(ProfileReadVar, String)> {
        if self.state != ProfileReadState::Current {
            return Vec::new();
        }
        let supplied: Vec<String> = supplied.into_iter().map(str::to_uppercase).collect();
        ProfileReadVar::ALL
            .into_iter()
            .filter(|var| !supplied.iter().any(|name| name == var.name()))
            .filter_map(|var| self.env.get(&var).map(|value| (var, value.clone())))
            .collect()
    }

    /// The error raised when a recorded grant cannot be
    /// used, or `None` when there is none or it is current.
    pub fn unusable(&self) -> Option<WindowsSandboxError> {
        if matches!(self.state, ProfileReadState::None | ProfileReadState::Current) {
            return None;
        }
        let why = self
            .reason
            .as_ref()
            .map_or_else(String::new, |reason| format!(": {reason}"));
        Some(WindowsSandboxError::new(
            Code::ProfileGrantOutdated,
            format!(
                "The recorded read grant on your user profile cannot be used ({}{why}). Run \
                 `fence-helper profile grant` (or `windows-install` again) as the user who starts \
                 sessions to bring it up to date, or `fence-helper profile forget` to run without \
                 reading the profile. Starting a session does neither itself.",
                self.state.as_str()
            ),
        ))
    }
}
