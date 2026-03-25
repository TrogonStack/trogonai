pub mod agent {
    pub fn initialize(prefix: &str) -> String {
        format!("{}.agent.initialize", prefix)
    }

    pub fn authenticate(prefix: &str) -> String {
        format!("{}.agent.authenticate", prefix)
    }

    pub fn session_new(prefix: &str) -> String {
        format!("{}.agent.session.new", prefix)
    }

    pub fn session_load(prefix: &str, session_id: &str) -> String {
        format!("{}.{}.agent.session.load", prefix, session_id)
    }

    pub fn session_cancel(prefix: &str, session_id: &str) -> String {
        format!("{}.{}.agent.session.cancel", prefix, session_id)
    }

    pub fn session_cancelled(prefix: &str, session_id: &str) -> String {
        format!("{}.{}.agent.session.cancelled", prefix, session_id)
    }

    pub fn session_set_mode(prefix: &str, session_id: &str) -> String {
        format!("{}.{}.agent.session.set_mode", prefix, session_id)
    }

    pub fn ext_session_ready(prefix: &str, session_id: &str) -> String {
        format!("{}.{}.agent.ext.session.ready", prefix, session_id)
    }

    pub fn session_prompt(prefix: &str, session_id: &str) -> String {
        format!("{}.{}.agent.session.prompt", prefix, session_id)
    }

    pub fn session_prompt_wildcard(prefix: &str) -> String {
        format!("{}.*.agent.session.prompt", prefix)
    }

    pub fn session_update(prefix: &str, session_id: &str, req_id: &str) -> String {
        format!("{}.{}.agent.session.update.{}", prefix, session_id, req_id)
    }

    pub fn ext_session_prompt_response(prefix: &str, session_id: &str, req_id: &str) -> String {
        format!(
            "{}.{}.agent.ext.session.prompt.response.{}",
            prefix, session_id, req_id
        )
    }

    pub fn session_list(prefix: &str) -> String {
        format!("{}.agent.session.list", prefix)
    }

    pub fn session_set_config_option(prefix: &str, session_id: &str) -> String {
        format!("{}.{}.agent.session.set_config_option", prefix, session_id)
    }

    pub fn session_set_model(prefix: &str, session_id: &str) -> String {
        format!("{}.{}.agent.session.set_model", prefix, session_id)
    }

    pub fn session_fork(prefix: &str, session_id: &str) -> String {
        format!("{}.{}.agent.session.fork", prefix, session_id)
    }

    pub fn session_resume(prefix: &str, session_id: &str) -> String {
        format!("{}.{}.agent.session.resume", prefix, session_id)
    }

    pub fn session_close(prefix: &str, session_id: &str) -> String {
        format!("{}.{}.agent.session.close", prefix, session_id)
    }

    pub fn ext(prefix: &str, method: &str) -> String {
        format!("{}.agent.ext.{}", prefix, method)
    }

    pub mod wildcards {
        pub fn all(prefix: &str) -> String {
            format!("{}.agent.>", prefix)
        }

        pub fn all_sessions(prefix: &str) -> String {
            format!("{}.*.agent.>", prefix)
        }
    }
}

pub mod client {
    pub fn fs_read_text_file(prefix: &str, session_id: &str) -> String {
        format!("{}.{}.client.fs.read_text_file", prefix, session_id)
    }

    pub fn fs_write_text_file(prefix: &str, session_id: &str) -> String {
        format!("{}.{}.client.fs.write_text_file", prefix, session_id)
    }

    pub fn session_request_permission(prefix: &str, session_id: &str) -> String {
        format!(
            "{}.{}.client.session.request_permission",
            prefix, session_id
        )
    }

    pub fn session_update(prefix: &str, session_id: &str) -> String {
        format!("{}.{}.client.session.update", prefix, session_id)
    }

    pub fn terminal_create(prefix: &str, session_id: &str) -> String {
        format!("{}.{}.client.terminal.create", prefix, session_id)
    }

    pub fn terminal_kill(prefix: &str, session_id: &str) -> String {
        format!("{}.{}.client.terminal.kill", prefix, session_id)
    }

    pub fn terminal_output(prefix: &str, session_id: &str) -> String {
        format!("{}.{}.client.terminal.output", prefix, session_id)
    }

    pub fn terminal_release(prefix: &str, session_id: &str) -> String {
        format!("{}.{}.client.terminal.release", prefix, session_id)
    }

    pub fn terminal_wait_for_exit(prefix: &str, session_id: &str) -> String {
        format!("{}.{}.client.terminal.wait_for_exit", prefix, session_id)
    }

    pub mod wildcards {
        pub fn all(prefix: &str) -> String {
            format!("{}.*.client.>", prefix)
        }
    }
}

pub mod wildcards {
    pub fn all(prefix: &str) -> String {
        format!("{}.>", prefix)
    }
}

#[cfg(test)]
mod tests {
    use super::{agent, client, wildcards};

    #[test]
    fn initialize_subject() {
        assert_eq!(agent::initialize("acp"), "acp.agent.initialize");
    }

    #[test]
    fn authenticate_subject() {
        assert_eq!(agent::authenticate("acp"), "acp.agent.authenticate");
    }

    #[test]
    fn session_new_subject() {
        assert_eq!(agent::session_new("acp"), "acp.agent.session.new");
    }

    #[test]
    fn session_load_subject() {
        assert_eq!(
            agent::session_load("acp", "s1"),
            "acp.s1.agent.session.load"
        );
    }

    #[test]
    fn session_cancel_subject() {
        assert_eq!(
            agent::session_cancel("acp", "s1"),
            "acp.s1.agent.session.cancel"
        );
    }

    #[test]
    fn session_set_mode_subject() {
        assert_eq!(
            agent::session_set_mode("acp", "s1"),
            "acp.s1.agent.session.set_mode"
        );
    }

    #[test]
    fn ext_session_ready_subject() {
        assert_eq!(
            agent::ext_session_ready("acp", "s1"),
            "acp.s1.agent.ext.session.ready"
        );
    }

    #[test]
    fn custom_prefix_places_prefix_at_root() {
        assert_eq!(agent::initialize("myapp"), "myapp.agent.initialize");
        assert_eq!(
            agent::session_load("myapp", "sess-42"),
            "myapp.sess-42.agent.session.load"
        );
        assert_eq!(
            agent::ext_session_ready("myapp", "sess-42"),
            "myapp.sess-42.agent.ext.session.ready"
        );
    }

    #[test]
    fn session_prompt_subject() {
        assert_eq!(
            agent::session_prompt("acp", "s1"),
            "acp.s1.agent.session.prompt"
        );
    }

    #[test]
    fn session_prompt_wildcard_subject() {
        assert_eq!(
            agent::session_prompt_wildcard("acp"),
            "acp.*.agent.session.prompt"
        );
    }

    #[test]
    fn session_update_subject() {
        assert_eq!(
            agent::session_update("acp", "s1", "req-abc"),
            "acp.s1.agent.session.update.req-abc"
        );
    }

    #[test]
    fn ext_session_prompt_response_subject() {
        assert_eq!(
            agent::ext_session_prompt_response("acp", "s1", "req-abc"),
            "acp.s1.agent.ext.session.prompt.response.req-abc"
        );
    }

    #[test]
    fn session_cancelled_subject() {
        assert_eq!(
            agent::session_cancelled("acp", "s1"),
            "acp.s1.agent.session.cancelled"
        );
    }

    #[test]
    fn session_list_subject() {
        assert_eq!(agent::session_list("acp"), "acp.agent.session.list");
    }

    #[test]
    fn session_set_config_option_subject() {
        assert_eq!(
            agent::session_set_config_option("acp", "s1"),
            "acp.s1.agent.session.set_config_option"
        );
    }

    #[test]
    fn session_set_model_subject() {
        assert_eq!(
            agent::session_set_model("acp", "s1"),
            "acp.s1.agent.session.set_model"
        );
    }

    #[test]
    fn session_fork_subject() {
        assert_eq!(
            agent::session_fork("acp", "s1"),
            "acp.s1.agent.session.fork"
        );
    }

    #[test]
    fn session_resume_subject() {
        assert_eq!(
            agent::session_resume("acp", "s1"),
            "acp.s1.agent.session.resume"
        );
    }

    #[test]
    fn session_close_subject() {
        assert_eq!(
            agent::session_close("acp", "s1"),
            "acp.s1.agent.session.close"
        );
    }

    #[test]
    fn session_scoped_subjects_share_token_layout() {
        let prefix = "acp";
        let sid = "abc";
        let expected_prefix = format!("{}.{}.agent.", prefix, sid);

        assert!(agent::session_load(prefix, sid).starts_with(&expected_prefix));
        assert!(agent::session_cancel(prefix, sid).starts_with(&expected_prefix));
        assert!(agent::session_set_mode(prefix, sid).starts_with(&expected_prefix));
        assert!(agent::session_set_config_option(prefix, sid).starts_with(&expected_prefix));
        assert!(agent::session_set_model(prefix, sid).starts_with(&expected_prefix));
        assert!(agent::session_fork(prefix, sid).starts_with(&expected_prefix));
        assert!(agent::session_resume(prefix, sid).starts_with(&expected_prefix));
        assert!(agent::session_close(prefix, sid).starts_with(&expected_prefix));
        assert!(agent::ext_session_ready(prefix, sid).starts_with(&expected_prefix));
    }

    #[test]
    fn ext_subject() {
        assert_eq!(agent::ext("acp", "my_tool"), "acp.agent.ext.my_tool");
    }

    #[test]
    fn agent_wildcard_all_subject() {
        assert_eq!(agent::wildcards::all("acp"), "acp.agent.>");
    }

    #[test]
    fn agent_wildcard_all_sessions_subject() {
        assert_eq!(agent::wildcards::all_sessions("acp"), "acp.*.agent.>");
    }

    #[test]
    fn agent_wildcards_overlap_when_session_id_is_agent() {
        let subject = "acp.agent.agent.session.load";
        let global = agent::wildcards::all("acp");
        let session = agent::wildcards::all_sessions("acp");

        // Both wildcards match this subject in NATS:
        // - "acp.agent.>" matches because "agent.session.load" falls under "acp.agent."
        // - "acp.*.agent.>" matches because * = "agent", rest = "session.load"
        // This is a known trade-off: using two subscriptions avoids consuming
        // client messages, but causes duplicate delivery for session ID "agent".
        assert!(nats_wildcard_matches(&global, subject));
        assert!(nats_wildcard_matches(&session, subject));
    }

    fn nats_wildcard_matches(pattern: &str, subject: &str) -> bool {
        let pattern_parts: Vec<&str> = pattern.split('.').collect();
        let subject_parts: Vec<&str> = subject.split('.').collect();
        nats_match(&pattern_parts, &subject_parts)
    }

    fn nats_match(pattern: &[&str], subject: &[&str]) -> bool {
        match (pattern.first(), subject.first()) {
            (Some(&">"), _) => true,
            (Some(&"*"), Some(_)) => nats_match(&pattern[1..], &subject[1..]),
            (Some(p), Some(s)) if p == s => nats_match(&pattern[1..], &subject[1..]),
            (None, None) => true,
            _ => false,
        }
    }

    #[test]
    fn nats_match_exact_match() {
        assert!(nats_wildcard_matches(
            "acp.agent.initialize",
            "acp.agent.initialize"
        ));
    }

    #[test]
    fn nats_match_no_match() {
        assert!(!nats_wildcard_matches(
            "acp.agent.initialize",
            "acp.agent.authenticate"
        ));
    }

    #[test]
    fn nats_match_length_mismatch() {
        assert!(!nats_wildcard_matches("acp.agent", "acp.agent.initialize"));
    }

    #[test]
    fn prefix_wildcard_all() {
        assert_eq!(wildcards::all("acp"), "acp.>");
    }

    #[test]
    fn prefix_wildcard_custom_prefix() {
        assert_eq!(wildcards::all("myapp"), "myapp.>");
    }

    #[test]
    fn agent_wildcard_custom_prefix() {
        assert_eq!(agent::wildcards::all("myapp"), "myapp.agent.>");
        assert_eq!(agent::wildcards::all_sessions("myapp"), "myapp.*.agent.>");
    }

    #[test]
    fn ext_subject_dotted_method() {
        assert_eq!(
            agent::ext("acp", "vendor.operation"),
            "acp.agent.ext.vendor.operation"
        );
    }

    #[test]
    fn ext_subject_custom_prefix() {
        assert_eq!(agent::ext("myapp", "my_tool"), "myapp.agent.ext.my_tool");
    }

    #[test]
    fn client_fs_read_text_file_subject() {
        assert_eq!(
            client::fs_read_text_file("acp", "s1"),
            "acp.s1.client.fs.read_text_file"
        );
    }

    #[test]
    fn client_fs_write_text_file_subject() {
        assert_eq!(
            client::fs_write_text_file("acp", "s1"),
            "acp.s1.client.fs.write_text_file"
        );
    }

    #[test]
    fn client_session_request_permission_subject() {
        assert_eq!(
            client::session_request_permission("acp", "s1"),
            "acp.s1.client.session.request_permission"
        );
    }

    #[test]
    fn client_session_update_subject() {
        assert_eq!(
            client::session_update("acp", "s1"),
            "acp.s1.client.session.update"
        );
    }

    #[test]
    fn client_terminal_create_subject() {
        assert_eq!(
            client::terminal_create("acp", "s1"),
            "acp.s1.client.terminal.create"
        );
    }

    #[test]
    fn client_terminal_kill_subject() {
        assert_eq!(
            client::terminal_kill("acp", "s1"),
            "acp.s1.client.terminal.kill"
        );
    }

    #[test]
    fn client_terminal_output_subject() {
        assert_eq!(
            client::terminal_output("acp", "s1"),
            "acp.s1.client.terminal.output"
        );
    }

    #[test]
    fn client_terminal_release_subject() {
        assert_eq!(
            client::terminal_release("acp", "s1"),
            "acp.s1.client.terminal.release"
        );
    }

    #[test]
    fn client_terminal_wait_for_exit_subject() {
        assert_eq!(
            client::terminal_wait_for_exit("acp", "s1"),
            "acp.s1.client.terminal.wait_for_exit"
        );
    }

    #[test]
    fn client_wildcard_all_subject() {
        assert_eq!(client::wildcards::all("acp"), "acp.*.client.>");
    }

    #[test]
    fn client_wildcard_custom_prefix() {
        assert_eq!(client::wildcards::all("myapp"), "myapp.*.client.>");
    }

    #[test]
    fn client_subjects_share_token_layout() {
        let prefix = "acp";
        let sid = "abc";
        let expected_prefix = format!("{}.{}.client.", prefix, sid);

        assert!(client::fs_read_text_file(prefix, sid).starts_with(&expected_prefix));
        assert!(client::fs_write_text_file(prefix, sid).starts_with(&expected_prefix));
        assert!(client::session_request_permission(prefix, sid).starts_with(&expected_prefix));
        assert!(client::session_update(prefix, sid).starts_with(&expected_prefix));
        assert!(client::terminal_create(prefix, sid).starts_with(&expected_prefix));
        assert!(client::terminal_kill(prefix, sid).starts_with(&expected_prefix));
        assert!(client::terminal_output(prefix, sid).starts_with(&expected_prefix));
        assert!(client::terminal_release(prefix, sid).starts_with(&expected_prefix));
        assert!(client::terminal_wait_for_exit(prefix, sid).starts_with(&expected_prefix));
    }

    #[test]
    fn client_subjects_custom_prefix() {
        assert_eq!(
            client::fs_read_text_file("myapp", "sess-42"),
            "myapp.sess-42.client.fs.read_text_file"
        );
        assert_eq!(
            client::terminal_create("myapp", "sess-42"),
            "myapp.sess-42.client.terminal.create"
        );
    }
}
