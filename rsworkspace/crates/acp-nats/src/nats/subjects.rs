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

    pub fn ext_session_ready(prefix: &str, session_id: &str) -> String {
        format!("{}.agent.ext.session.ready.{}", prefix, session_id)
    }
}
