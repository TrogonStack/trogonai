//! Session metadata readers that pick the summary rules of the selected upstream SDK.
//!
//! The TypeScript rules live in `session_files_ts` and the Python rules in `session_files`; both build on the shared
//! lite reads of `session_files`, so the choice between them is made here, above both.

use std::path::{Path, PathBuf};

use crate::{
    ListSessionsOptions, SdkSessionInfo, SessionFiles, SummaryDialect,
    constants::PRECOMPACT_SKIP_THRESHOLD,
    session_files::{collect_agent_files, read_agent_metadata_sidecar, valid_uuid},
    session_messages_ts::{ChainAlgorithm, MessageOptions, MessagesError, session_messages, subagent_messages},
    session_transcript::{
        MessagePage, SessionMessage, entries_to_session_messages, entries_to_subagent_messages,
        parent_ids_from_agent_metadata, parse_transcript_entries,
    },
};

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

    /// Reads a session's visible conversation under the TypeScript rules, failing exactly where the TypeScript
    /// pipeline throws. Select [`ChainAlgorithm::Python`] through [`SessionFiles::get_session_messages_with`] for the
    /// Python reader, which never fails.
    pub fn get_session_messages(
        &self,
        session_id: &str,
        directory: Option<&Path>,
        page: MessagePage,
    ) -> Result<Vec<SessionMessage>, MessagesError> {
        self.get_session_messages_with(
            session_id,
            directory,
            MessageOptions {
                page,
                ..MessageOptions::default()
            },
        )
    }

    /// [`SessionFiles::get_session_messages`] under the selected upstream SDK's rules and options.
    pub fn get_session_messages_with(
        &self,
        session_id: &str,
        directory: Option<&Path>,
        options: MessageOptions,
    ) -> Result<Vec<SessionMessage>, MessagesError> {
        let Some(session_id) = valid_uuid(session_id) else {
            return Ok(Vec::new());
        };
        if options.algorithm == ChainAlgorithm::TypeScript {
            let Some((path, _)) = self.resolve_session_ts(session_id, directory) else {
                return Ok(Vec::new());
            };
            let Some(content) = read_transcript_ts(&path, self.precompact_skip) else {
                return Ok(Vec::new());
            };
            return session_messages(&content, options.include_system_messages, options.page);
        }
        let Some(content) = self.read_session_file(session_id, directory) else {
            return Ok(Vec::new());
        };
        Ok(entries_to_session_messages(
            &parse_transcript_entries(&content),
            options.page,
        ))
    }

    /// Lists the ids of the subagents of a session, including nested `subagents/workflows/<run>/` transcripts.
    pub fn list_subagents(&self, session_id: &str, directory: Option<&Path>) -> Vec<String> {
        self.list_subagents_with(session_id, directory, ChainAlgorithm::TypeScript)
    }

    /// [`SessionFiles::list_subagents`] with the session located by the selected SDK's directory rules.
    pub fn list_subagents_with(
        &self,
        session_id: &str,
        directory: Option<&Path>,
        algorithm: ChainAlgorithm,
    ) -> Vec<String> {
        let Some(session_id) = valid_uuid(session_id) else {
            return Vec::new();
        };
        let Some(subagents) = self.resolve_subagents_dir(session_id, directory, algorithm) else {
            return Vec::new();
        };
        collect_agent_files(&subagents).into_iter().map(|(id, _)| id).collect()
    }

    /// Reads a subagent's conversation. The parent ids come from the `agent-<id>.meta.json` sidecar and are `None`
    /// when it is missing or unusable.
    pub fn get_subagent_messages(
        &self,
        session_id: &str,
        agent_id: &str,
        directory: Option<&Path>,
        page: MessagePage,
    ) -> Vec<SessionMessage> {
        self.get_subagent_messages_with(
            session_id,
            agent_id,
            directory,
            MessageOptions {
                page,
                ..MessageOptions::default()
            },
        )
    }

    /// [`SessionFiles::get_subagent_messages`] under the selected upstream SDK's rules.
    pub fn get_subagent_messages_with(
        &self,
        session_id: &str,
        agent_id: &str,
        directory: Option<&Path>,
        options: MessageOptions,
    ) -> Vec<SessionMessage> {
        let Some(session_id) = valid_uuid(session_id) else {
            return Vec::new();
        };
        if agent_id.is_empty() {
            return Vec::new();
        }
        let Some(subagents) = self.resolve_subagents_dir(session_id, directory, options.algorithm) else {
            return Vec::new();
        };
        let Some((_, transcript)) = collect_agent_files(&subagents)
            .into_iter()
            .find(|(id, _)| id == agent_id)
        else {
            return Vec::new();
        };
        let meta = read_agent_metadata_sidecar(&transcript);
        let (tool_use_id, parent_agent_id) = parent_ids_from_agent_metadata(meta.as_ref());
        if options.algorithm == ChainAlgorithm::TypeScript {
            let Ok(content) = std::fs::read(&transcript) else {
                return Vec::new();
            };
            return subagent_messages(
                &content,
                options.page,
                tool_use_id.as_deref(),
                parent_agent_id.as_deref(),
            );
        }
        let Ok(content) = std::fs::read_to_string(&transcript) else {
            return Vec::new();
        };
        if content.is_empty() {
            return Vec::new();
        }
        entries_to_subagent_messages(
            &parse_transcript_entries(&content),
            options.page,
            tool_use_id.as_deref(),
            parent_agent_id.as_deref(),
        )
    }

    fn resolve_subagents_dir(
        &self,
        session_id: &str,
        directory: Option<&Path>,
        algorithm: ChainAlgorithm,
    ) -> Option<PathBuf> {
        let transcript = match algorithm {
            ChainAlgorithm::TypeScript => self.resolve_session_ts(session_id, directory)?.0,
            ChainAlgorithm::Python => self.resolve_session_file_path(session_id, directory)?,
        };
        Some(transcript.with_extension("").join("subagents"))
    }
}

/// The transcript bytes as `qIe` reads them.
fn read_transcript_ts(path: &Path, precompact_skip: bool) -> Option<Vec<u8>> {
    let size = std::fs::metadata(path).ok()?.len();
    let content = if size > PRECOMPACT_SKIP_THRESHOLD && precompact_skip {
        crate::session_precompact::read_after_last_boundary(path, size)?
    } else {
        std::fs::read(path).ok()?
    };
    (!content.is_empty()).then_some(content)
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

/// [`SessionFiles::get_session_messages`] over the environment's config directory.
pub fn get_session_messages(
    session_id: &str,
    directory: Option<&Path>,
    page: MessagePage,
) -> Result<Vec<SessionMessage>, MessagesError> {
    match SessionFiles::from_environment() {
        Some(files) => files.get_session_messages(session_id, directory, page),
        None => Ok(Vec::new()),
    }
}

/// [`SessionFiles::list_subagents`] over the environment's config directory.
pub fn list_subagents(session_id: &str, directory: Option<&Path>) -> Vec<String> {
    SessionFiles::from_environment()
        .map(|files| files.list_subagents(session_id, directory))
        .unwrap_or_default()
}

/// [`SessionFiles::get_subagent_messages`] over the environment's config directory.
pub fn get_subagent_messages(
    session_id: &str,
    agent_id: &str,
    directory: Option<&Path>,
    page: MessagePage,
) -> Vec<SessionMessage> {
    SessionFiles::from_environment()
        .map(|files| files.get_subagent_messages(session_id, agent_id, directory, page))
        .unwrap_or_default()
}
