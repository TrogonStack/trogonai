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

    pub fn session_set_model(prefix: &str, session_id: &str) -> String {
        format!("{}.{}.agent.session.set_model", prefix, session_id)
    }

    pub fn session_set_config_option(prefix: &str, session_id: &str) -> String {
        format!("{}.{}.agent.session.set_config_option", prefix, session_id)
    }

    pub fn session_list(prefix: &str) -> String {
        format!("{}.agent.session.list", prefix)
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

    /// Alias for `session_prompt` — used by the runner crate.
    pub fn prompt(prefix: &str, session_id: &str) -> String {
        session_prompt(prefix, session_id)
    }

    /// Alias for `session_prompt_wildcard` — used by the runner crate.
    pub fn prompt_wildcard(prefix: &str) -> String {
        session_prompt_wildcard(prefix)
    }

    /// Alias for `session_update` — used by the runner crate.
    pub fn prompt_events(prefix: &str, session_id: &str, req_id: &str) -> String {
        session_update(prefix, session_id, req_id)
    }

    pub fn ext(prefix: &str, method: &str) -> String {
        format!("{}.agent.ext.{}", prefix, method)
    }
}

pub mod client {
    pub mod wildcards {
        pub fn all(prefix: &str) -> String {
            format!("{}.*.client.>", prefix)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{agent, client};

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
    fn session_set_model_subject() {
        assert_eq!(
            agent::session_set_model("acp", "s1"),
            "acp.s1.agent.session.set_model"
        );
    }

    #[test]
    fn session_set_config_option_subject() {
        assert_eq!(
            agent::session_set_config_option("acp", "s1"),
            "acp.s1.agent.session.set_config_option"
        );
    }

    #[test]
    fn session_list_subject() {
        assert_eq!(agent::session_list("acp"), "acp.agent.session.list");
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
    fn client_wildcard_all_subject() {
        assert_eq!(client::wildcards::all("acp"), "acp.*.client.>");
    }

    #[test]
    fn prompt_alias_matches_session_prompt() {
        assert_eq!(
            agent::prompt("acp", "s1"),
            agent::session_prompt("acp", "s1")
        );
    }

    #[test]
    fn prompt_wildcard_alias_matches_session_prompt_wildcard() {
        assert_eq!(
            agent::prompt_wildcard("acp"),
            agent::session_prompt_wildcard("acp")
        );
    }

    #[test]
    fn prompt_events_alias_matches_session_update() {
        assert_eq!(
            agent::prompt_events("acp", "s1", "r1"),
            agent::session_update("acp", "s1", "r1")
        );
    }
}
