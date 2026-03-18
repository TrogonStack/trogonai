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

    /// Broadcast subject published when a session is cancelled.
    /// All bridge `prompt()` callers for this session subscribe here and
    /// drain immediately when they receive a message.
    pub fn session_cancelled(prefix: &str, session_id: &str) -> String {
        format!("{}.{}.agent.session.cancelled", prefix, session_id)
    }

    pub fn session_set_mode(prefix: &str, session_id: &str) -> String {
        format!("{}.{}.agent.session.set_mode", prefix, session_id)
    }

    pub fn ext_session_ready(prefix: &str, session_id: &str) -> String {
        format!("{}.{}.agent.ext.session.ready", prefix, session_id)
    }

    /// Subject the Bridge publishes prompt payloads to.
    /// The Runner subscribes here via `{prefix}.*.agent.prompt`.
    pub fn prompt(prefix: &str, session_id: &str) -> String {
        format!("{}.{}.agent.prompt", prefix, session_id)
    }

    /// Wildcard subject the Runner uses to subscribe to all prompt payloads.
    pub fn prompt_wildcard(prefix: &str) -> String {
        format!("{}.*.agent.prompt", prefix)
    }

    /// Subject the Runner publishes per-event streaming updates to.
    /// The Bridge subscribes here while waiting for the turn to complete.
    pub fn prompt_events(prefix: &str, session_id: &str, req_id: &str) -> String {
        format!("{}.{}.agent.prompt.events.{}", prefix, session_id, req_id)
    }
}

#[cfg(test)]
mod tests {
    use super::agent;

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
    fn prompt_subject() {
        assert_eq!(agent::prompt("acp", "s1"), "acp.s1.agent.prompt");
    }

    #[test]
    fn prompt_wildcard_subject() {
        assert_eq!(agent::prompt_wildcard("acp"), "acp.*.agent.prompt");
    }

    #[test]
    fn prompt_events_subject() {
        assert_eq!(
            agent::prompt_events("acp", "s1", "req-abc"),
            "acp.s1.agent.prompt.events.req-abc"
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
