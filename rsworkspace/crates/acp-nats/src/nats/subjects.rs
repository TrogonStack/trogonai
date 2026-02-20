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

    pub fn ext(prefix: &str, method: &str) -> String {
        format!("{}.agent.ext.{}", prefix, method)
    }

    pub fn ext_session(prefix: &str, session_id: &str, method: &str) -> String {
        format!("{}.{}.agent.ext.{}", prefix, session_id, method)
    }

    /// Signals that session/new or session/load response has been sent
    pub fn ext_session_ready(prefix: &str, session_id: &str) -> String {
        format!("{}.{}.agent.ext.session.ready", prefix, session_id)
    }

    pub mod wildcards {
        pub fn all(prefix: &str) -> String {
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
