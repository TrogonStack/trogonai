//! Session metadata readers that pick the summary rules of the selected upstream SDK.
//!
//! The TypeScript rules live in `session_files_ts` and the Python rules in `session_files`; both build on the shared
//! lite reads of `session_files`, so the choice between them is made here, above both.

use std::path::Path;

use crate::{ListSessionsOptions, SdkSessionInfo, SessionFiles, SummaryDialect};

impl SessionFiles {
    /// Lists sessions newest first, with ties broken by session id so pages are stable.
    pub fn list_sessions(&self, options: &ListSessionsOptions) -> Vec<SdkSessionInfo> {
        if options.dialect == SummaryDialect::TypeScript {
            return self.list_sessions_ts(options);
        }
        self.list_sessions_python(options)
    }

    /// Reads one session's metadata, or `None` when it is missing, a sidechain, or has nothing to summarize.
    pub fn get_session_info(&self, session_id: &str, directory: Option<&Path>) -> Option<SdkSessionInfo> {
        self.get_session_info_with(session_id, directory, SummaryDialect::TypeScript)
    }

    /// [`SessionFiles::get_session_info`] under the selected upstream SDK's rules.
    pub fn get_session_info_with(
        &self,
        session_id: &str,
        directory: Option<&Path>,
        dialect: SummaryDialect,
    ) -> Option<SdkSessionInfo> {
        if dialect == SummaryDialect::TypeScript {
            return self.get_session_info_ts(session_id, directory);
        }
        self.get_session_info_python(session_id, directory)
    }
}

/// [`SessionFiles::list_sessions`] over the environment's config directory; empty when none can be resolved.
pub fn list_sessions(options: &ListSessionsOptions) -> Vec<SdkSessionInfo> {
    SessionFiles::from_environment()
        .map(|files| files.list_sessions(options))
        .unwrap_or_default()
}

/// [`SessionFiles::get_session_info`] over the environment's config directory.
pub fn get_session_info(session_id: &str, directory: Option<&Path>) -> Option<SdkSessionInfo> {
    SessionFiles::from_environment()?.get_session_info(session_id, directory)
}
