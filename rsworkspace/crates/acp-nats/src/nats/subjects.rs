//! NATS subject definitions for ACP protocol.
//!
//! The prefix token (default: "acp") can be customized for multi-tenancy or deployment isolation.
//! The prefix completely replaces "acp" as the root token in the subject hierarchy.

/// Agent-side subjects (IDE → Backend)
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

    /// Notifications and request/reply ext methods share the same NATS subject
    /// (`{prefix}.agent.ext.{method}`). This is safe because notifications use
    /// fire-and-forget `publish` while methods use `request` (which sets a reply
    /// subject). A backend subscriber can distinguish the two by checking whether
    /// the message has a reply address.
    pub fn ext(prefix: &str, method: &str) -> String {
        format!("{}.agent.ext.{}", prefix, method)
    }

    /// Signals that session/new or session/load response has been sent
    pub fn ext_session_ready(prefix: &str, session_id: &str) -> String {
        format!("{}.{}.agent.ext.session.ready", prefix, session_id)
    }

    pub mod wildcards {
        /// Matches only non-session agent subjects (e.g. initialize, authenticate).
        pub fn global(prefix: &str) -> String {
            format!("{}.agent.>", prefix)
        }

        /// Matches all session-scoped agent subjects across all sessions.
        ///
        /// This intentionally includes extension subjects (`agent.ext.*`) in addition to
        /// session request/reply subjects. Callers that need stricter matching should filter
        /// by subject shape or method token explicitly.
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
        assert!(agent::session_cancel(prefix, sid).starts_with(&expected_prefix));
        assert!(agent::session_set_mode(prefix, sid).starts_with(&expected_prefix));
        assert!(agent::ext_session_ready(prefix, sid).starts_with(&expected_prefix));
    }
}
