//! The module level session helpers resolve the config directory from the process environment like Python's
//! `_get_claude_config_home_dir`, and report a missing home directory the way Python does: as an error, never as an
//! empty result. These tests read the ambient process environment, so each one accepts both outcomes the host can
//! produce and pins what each outcome must look like.

use std::path::Path;

use trogon_claude_agent_sdk::{
    HomeDirectoryError, ListSessionsOptions, MessagePage, MessagesError, SessionFiles, SessionMutationError,
    delete_session, fork_session, get_session_info, get_session_messages, get_subagent_messages, list_sessions,
    list_subagents, rename_session, tag_session,
};

const PYTHON_MESSAGE: &str = "Could not determine home directory.";
const UNKNOWN: &str = "not-a-session-id";

fn missing_home(error: &HomeDirectoryError) {
    assert_eq!(error.message(), PYTHON_MESSAGE);
    assert_eq!(error.to_string(), PYTHON_MESSAGE);
}

fn mutation_outcome(result: Result<(), SessionMutationError>) {
    match result {
        Err(SessionMutationError::InvalidSessionId(id)) => assert_eq!(id, UNKNOWN),
        Err(SessionMutationError::HomeDirectory(error)) => missing_home(&error),
        other => panic!("unexpected outcome {other:?}"),
    }
}

#[test]
fn the_environment_constructor_reports_a_missing_home_as_the_python_error() {
    match SessionFiles::from_environment() {
        Ok(files) => assert!(files.projects_dir().ends_with("projects")),
        Err(error) => missing_home(&error),
    }
}

#[test]
fn listing_sessions_never_turns_a_missing_home_into_an_empty_list() {
    let options = ListSessionsOptions {
        directory: Some(Path::new("/nonexistent-project-for-the-home-directory-tests").to_path_buf()),
        include_worktrees: false,
        ..ListSessionsOptions::default()
    };
    match list_sessions(&options) {
        Ok(sessions) => assert!(sessions.is_empty()),
        Err(error) => missing_home(&error),
    }
}

#[test]
fn reading_one_session_never_turns_a_missing_home_into_none() {
    match get_session_info(UNKNOWN, None) {
        Ok(info) => assert!(info.is_none()),
        Err(error) => missing_home(&error),
    }
}

#[test]
fn reading_messages_reports_a_missing_home_through_the_messages_error() {
    match get_session_messages(UNKNOWN, None, MessagePage::default()) {
        Ok(messages) => assert!(messages.is_empty()),
        Err(MessagesError::HomeDirectory(error)) => missing_home(&error),
        Err(other) => panic!("unexpected error {other:?}"),
    }
}

#[test]
fn subagent_readers_never_turn_a_missing_home_into_an_empty_result() {
    match list_subagents(UNKNOWN, None) {
        Ok(agents) => assert!(agents.is_empty()),
        Err(error) => missing_home(&error),
    }
    match get_subagent_messages(UNKNOWN, "agent", None, MessagePage::default()) {
        Ok(messages) => assert!(messages.is_empty()),
        Err(error) => missing_home(&error),
    }
}

#[test]
fn mutations_report_a_missing_home_instead_of_a_missing_session() {
    mutation_outcome(rename_session(UNKNOWN, "title", None));
    mutation_outcome(tag_session(UNKNOWN, Some("tag"), None));
    mutation_outcome(delete_session(UNKNOWN, None));
    mutation_outcome(fork_session(UNKNOWN, None, None, None).map(|_| ()));
}
