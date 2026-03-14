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

    pub fn session_prompt(prefix: &str, session_id: &str) -> String {
        format!("{}.{}.agent.session.prompt", prefix, session_id)
    }

    pub fn session_cancel(prefix: &str, session_id: &str) -> String {
        format!("{}.{}.agent.session.cancel", prefix, session_id)
    }

    pub fn session_set_mode(prefix: &str, session_id: &str) -> String {
        format!("{}.{}.agent.session.set_mode", prefix, session_id)
    }

    /// Extension methods and notifications share subject `{prefix}.agent.ext.{method}`.
    /// Backend distinguishes request (reply expected) vs notification (fire-and-forget) by reply address.
    pub fn ext(prefix: &str, method: &str) -> String {
        format!("{}.agent.ext.{}", prefix, method)
    }

    pub fn ext_session_ready(prefix: &str, session_id: &str) -> String {
        format!("{}.{}.agent.ext.session.ready", prefix, session_id)
    }

    pub mod wildcards {
        pub fn global(prefix: &str) -> String {
            format!("{}.agent.>", prefix)
        }

        pub fn all_sessions(prefix: &str) -> String {
            format!("{}.*.agent.>", prefix)
        }

        pub fn session(prefix: &str, session_id: &str) -> String {
            format!("{}.{}.agent.>", prefix, session_id)
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

    pub fn ext(prefix: &str, session_id: &str, method: &str) -> String {
        format!("{}.{}.client.ext.{}", prefix, session_id, method)
    }

    pub fn ext_session_prompt_response(prefix: &str, session_id: &str) -> String {
        ext(prefix, session_id, "session.prompt_response")
    }

    pub mod wildcards {
        pub fn all(prefix: &str) -> String {
            format!("{}.*.client.>", prefix)
        }

        pub fn session(prefix: &str, session_id: &str) -> String {
            format!("{}.{}.client.>", prefix, session_id)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::agent;
    use super::client;

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
    fn client_ext_session_prompt_response_subject() {
        assert_eq!(
            client::ext_session_prompt_response("acp", "s1"),
            "acp.s1.client.ext.session.prompt_response"
        );
    }

    #[test]
    fn client_wildcards_all() {
        assert_eq!(client::wildcards::all("foo"), "foo.*.client.>");
    }

    #[test]
    fn agent_wildcards_global() {
        assert_eq!(agent::wildcards::global("acp"), "acp.agent.>");
    }

    #[test]
    fn agent_wildcards_all_sessions() {
        assert_eq!(agent::wildcards::all_sessions("acp"), "acp.*.agent.>");
    }

    #[test]
    fn agent_wildcards_session() {
        assert_eq!(agent::wildcards::session("acp", "s1"), "acp.s1.agent.>");
    }

    #[test]
    fn client_wildcards_session() {
        assert_eq!(client::wildcards::session("acp", "s1"), "acp.s1.client.>");
    }

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
    fn session_prompt_subject() {
        assert_eq!(
            agent::session_prompt("acp", "s1"),
            "acp.s1.agent.session.prompt"
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
    fn ext_subject() {
        assert_eq!(agent::ext("acp", "my_method"), "acp.agent.ext.my_method");
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
    fn session_scoped_subjects_share_token_layout() {
        let prefix = "acp";
        let sid = "abc";
        let expected_prefix = format!("{}.{}.agent.", prefix, sid);

        assert!(agent::session_load(prefix, sid).starts_with(&expected_prefix));
        assert!(agent::session_prompt(prefix, sid).starts_with(&expected_prefix));
        assert!(agent::session_cancel(prefix, sid).starts_with(&expected_prefix));
        assert!(agent::session_set_mode(prefix, sid).starts_with(&expected_prefix));
        assert!(agent::ext_session_ready(prefix, sid).starts_with(&expected_prefix));
    }

    #[test]
    fn multi_tenant_prefixes_are_isolated() {
        let sid = "sess123";
        assert_ne!(agent::initialize("tenant1"), agent::initialize("tenant2"));
        assert_ne!(
            agent::session_prompt("tenant1", sid),
            agent::session_prompt("tenant2", sid),
        );
    }

    #[test]
    fn environment_based_prefixes() {
        assert_eq!(agent::initialize("dev"), "dev.agent.initialize");
        assert_eq!(agent::initialize("prod"), "prod.agent.initialize");
        assert_eq!(agent::initialize("staging"), "staging.agent.initialize");
    }

    #[test]
    fn special_characters_in_prefix() {
        assert_eq!(agent::initialize("my_app"), "my_app.agent.initialize");
        assert_eq!(agent::initialize("my-app"), "my-app.agent.initialize");
        assert_eq!(agent::initialize("app123"), "app123.agent.initialize");
    }

    #[test]
    fn prefix_does_not_add_extra_namespace() {
        assert_eq!(agent::initialize("myapp"), "myapp.agent.initialize");
        assert_ne!(agent::initialize("myapp"), "myapp.acp.agent.initialize");
    }
}
