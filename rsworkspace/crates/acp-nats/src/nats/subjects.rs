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

    pub fn session_list(prefix: &str) -> String {
        format!("{}.agent.session.list", prefix)
    }

    pub fn ext(prefix: &str, method: &str) -> String {
        format!("{}.agent.ext.{}", prefix, method)
    }

    pub mod wildcards {
        pub fn all(prefix: &str) -> String {
            format!("{}.agent.>", prefix)
        }
    }
}

pub mod session {
    pub mod agent {
        pub fn load(prefix: &str, session_id: &str) -> String {
            format!("{}.session.{}.agent.load", prefix, session_id)
        }

        pub fn prompt(prefix: &str, session_id: &str) -> String {
            format!("{}.session.{}.agent.prompt", prefix, session_id)
        }

        pub fn prompt_wildcard(prefix: &str) -> String {
            format!("{}.session.*.agent.prompt", prefix)
        }

        pub fn cancel(prefix: &str, session_id: &str) -> String {
            format!("{}.session.{}.agent.cancel", prefix, session_id)
        }

        pub fn cancelled(prefix: &str, session_id: &str) -> String {
            format!("{}.session.{}.agent.cancelled", prefix, session_id)
        }

        pub fn set_mode(prefix: &str, session_id: &str) -> String {
            format!("{}.session.{}.agent.set_mode", prefix, session_id)
        }

        pub fn set_config_option(prefix: &str, session_id: &str) -> String {
            format!("{}.session.{}.agent.set_config_option", prefix, session_id)
        }

        pub fn set_model(prefix: &str, session_id: &str) -> String {
            format!("{}.session.{}.agent.set_model", prefix, session_id)
        }

        pub fn fork(prefix: &str, session_id: &str) -> String {
            format!("{}.session.{}.agent.fork", prefix, session_id)
        }

        pub fn resume(prefix: &str, session_id: &str) -> String {
            format!("{}.session.{}.agent.resume", prefix, session_id)
        }

        pub fn close(prefix: &str, session_id: &str) -> String {
            format!("{}.session.{}.agent.close", prefix, session_id)
        }

        pub fn ext_ready(prefix: &str, session_id: &str) -> String {
            format!("{}.session.{}.agent.ext.ready", prefix, session_id)
        }

        pub fn update(prefix: &str, session_id: &str, req_id: &str) -> String {
            format!("{}.session.{}.agent.update.{}", prefix, session_id, req_id)
        }

        pub fn prompt_response(prefix: &str, session_id: &str, req_id: &str) -> String {
            format!(
                "{}.session.{}.agent.prompt.response.{}",
                prefix, session_id, req_id
            )
        }

        pub fn response(prefix: &str, session_id: &str, req_id: &str) -> String {
            format!(
                "{}.session.{}.agent.response.{}",
                prefix, session_id, req_id
            )
        }
    }

    pub mod client {
        pub fn fs_read_text_file(prefix: &str, session_id: &str) -> String {
            format!("{}.session.{}.client.fs.read_text_file", prefix, session_id)
        }

        pub fn fs_write_text_file(prefix: &str, session_id: &str) -> String {
            format!(
                "{}.session.{}.client.fs.write_text_file",
                prefix, session_id
            )
        }

        pub fn session_request_permission(prefix: &str, session_id: &str) -> String {
            format!(
                "{}.session.{}.client.session.request_permission",
                prefix, session_id
            )
        }

        pub fn session_update(prefix: &str, session_id: &str) -> String {
            format!("{}.session.{}.client.session.update", prefix, session_id)
        }

        pub fn terminal_create(prefix: &str, session_id: &str) -> String {
            format!("{}.session.{}.client.terminal.create", prefix, session_id)
        }

        pub fn terminal_kill(prefix: &str, session_id: &str) -> String {
            format!("{}.session.{}.client.terminal.kill", prefix, session_id)
        }

        pub fn terminal_output(prefix: &str, session_id: &str) -> String {
            format!("{}.session.{}.client.terminal.output", prefix, session_id)
        }

        pub fn terminal_release(prefix: &str, session_id: &str) -> String {
            format!("{}.session.{}.client.terminal.release", prefix, session_id)
        }

        pub fn terminal_wait_for_exit(prefix: &str, session_id: &str) -> String {
            format!(
                "{}.session.{}.client.terminal.wait_for_exit",
                prefix, session_id
            )
        }
    }

    pub mod wildcards {
        pub fn all(prefix: &str) -> String {
            format!("{}.session.>", prefix)
        }

        pub fn one(prefix: &str, session_id: &str) -> String {
            format!("{}.session.{}.>", prefix, session_id)
        }

        pub fn all_agent(prefix: &str) -> String {
            format!("{}.session.*.agent.>", prefix)
        }

        pub fn all_client(prefix: &str) -> String {
            format!("{}.session.*.client.>", prefix)
        }

        pub fn one_agent(prefix: &str, session_id: &str) -> String {
            format!("{}.session.{}.agent.>", prefix, session_id)
        }

        pub fn one_client(prefix: &str, session_id: &str) -> String {
            format!("{}.session.{}.client.>", prefix, session_id)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{agent, session};

    #[test]
    fn agent_initialize() {
        assert_eq!(agent::initialize("acp"), "acp.agent.initialize");
    }

    #[test]
    fn agent_authenticate() {
        assert_eq!(agent::authenticate("acp"), "acp.agent.authenticate");
    }

    #[test]
    fn agent_session_new() {
        assert_eq!(agent::session_new("acp"), "acp.agent.session.new");
    }

    #[test]
    fn agent_session_list() {
        assert_eq!(agent::session_list("acp"), "acp.agent.session.list");
    }

    #[test]
    fn agent_ext() {
        assert_eq!(agent::ext("acp", "my_tool"), "acp.agent.ext.my_tool");
    }

    #[test]
    fn agent_ext_dotted() {
        assert_eq!(agent::ext("acp", "vendor.op"), "acp.agent.ext.vendor.op");
    }

    #[test]
    fn agent_wildcard_all() {
        assert_eq!(agent::wildcards::all("acp"), "acp.agent.>");
    }

    #[test]
    fn session_agent_load() {
        assert_eq!(
            session::agent::load("acp", "s1"),
            "acp.session.s1.agent.load"
        );
    }

    #[test]
    fn session_agent_prompt() {
        assert_eq!(
            session::agent::prompt("acp", "s1"),
            "acp.session.s1.agent.prompt"
        );
    }

    #[test]
    fn session_agent_prompt_wildcard() {
        assert_eq!(
            session::agent::prompt_wildcard("acp"),
            "acp.session.*.agent.prompt"
        );
    }

    #[test]
    fn session_agent_cancel() {
        assert_eq!(
            session::agent::cancel("acp", "s1"),
            "acp.session.s1.agent.cancel"
        );
    }

    #[test]
    fn session_agent_cancelled() {
        assert_eq!(
            session::agent::cancelled("acp", "s1"),
            "acp.session.s1.agent.cancelled"
        );
    }

    #[test]
    fn session_agent_set_mode() {
        assert_eq!(
            session::agent::set_mode("acp", "s1"),
            "acp.session.s1.agent.set_mode"
        );
    }

    #[test]
    fn session_agent_set_config_option() {
        assert_eq!(
            session::agent::set_config_option("acp", "s1"),
            "acp.session.s1.agent.set_config_option"
        );
    }

    #[test]
    fn session_agent_set_model() {
        assert_eq!(
            session::agent::set_model("acp", "s1"),
            "acp.session.s1.agent.set_model"
        );
    }

    #[test]
    fn session_agent_fork() {
        assert_eq!(
            session::agent::fork("acp", "s1"),
            "acp.session.s1.agent.fork"
        );
    }

    #[test]
    fn session_agent_resume() {
        assert_eq!(
            session::agent::resume("acp", "s1"),
            "acp.session.s1.agent.resume"
        );
    }

    #[test]
    fn session_agent_close() {
        assert_eq!(
            session::agent::close("acp", "s1"),
            "acp.session.s1.agent.close"
        );
    }

    #[test]
    fn session_agent_ext_ready() {
        assert_eq!(
            session::agent::ext_ready("acp", "s1"),
            "acp.session.s1.agent.ext.ready"
        );
    }

    #[test]
    fn session_agent_update() {
        assert_eq!(
            session::agent::update("acp", "s1", "req-abc"),
            "acp.session.s1.agent.update.req-abc"
        );
    }

    #[test]
    fn session_agent_prompt_response() {
        assert_eq!(
            session::agent::prompt_response("acp", "s1", "req-abc"),
            "acp.session.s1.agent.prompt.response.req-abc"
        );
    }

    #[test]
    fn session_agent_response() {
        assert_eq!(
            session::agent::response("acp", "s1", "req-abc"),
            "acp.session.s1.agent.response.req-abc"
        );
    }

    #[test]
    fn session_client_fs_read() {
        assert_eq!(
            session::client::fs_read_text_file("acp", "s1"),
            "acp.session.s1.client.fs.read_text_file"
        );
    }

    #[test]
    fn session_client_fs_write() {
        assert_eq!(
            session::client::fs_write_text_file("acp", "s1"),
            "acp.session.s1.client.fs.write_text_file"
        );
    }

    #[test]
    fn session_client_request_permission() {
        assert_eq!(
            session::client::session_request_permission("acp", "s1"),
            "acp.session.s1.client.session.request_permission"
        );
    }

    #[test]
    fn session_client_session_update() {
        assert_eq!(
            session::client::session_update("acp", "s1"),
            "acp.session.s1.client.session.update"
        );
    }

    #[test]
    fn session_client_terminal_create() {
        assert_eq!(
            session::client::terminal_create("acp", "s1"),
            "acp.session.s1.client.terminal.create"
        );
    }

    #[test]
    fn session_client_terminal_kill() {
        assert_eq!(
            session::client::terminal_kill("acp", "s1"),
            "acp.session.s1.client.terminal.kill"
        );
    }

    #[test]
    fn session_client_terminal_output() {
        assert_eq!(
            session::client::terminal_output("acp", "s1"),
            "acp.session.s1.client.terminal.output"
        );
    }

    #[test]
    fn session_client_terminal_release() {
        assert_eq!(
            session::client::terminal_release("acp", "s1"),
            "acp.session.s1.client.terminal.release"
        );
    }

    #[test]
    fn session_client_terminal_wait_for_exit() {
        assert_eq!(
            session::client::terminal_wait_for_exit("acp", "s1"),
            "acp.session.s1.client.terminal.wait_for_exit"
        );
    }

    #[test]
    fn session_wildcard_all() {
        assert_eq!(session::wildcards::all("acp"), "acp.session.>");
    }

    #[test]
    fn session_wildcard_one() {
        assert_eq!(session::wildcards::one("acp", "s1"), "acp.session.s1.>");
    }

    #[test]
    fn session_wildcard_all_agent() {
        assert_eq!(
            session::wildcards::all_agent("acp"),
            "acp.session.*.agent.>"
        );
    }

    #[test]
    fn session_wildcard_all_client() {
        assert_eq!(
            session::wildcards::all_client("acp"),
            "acp.session.*.client.>"
        );
    }

    #[test]
    fn session_wildcard_one_agent() {
        assert_eq!(
            session::wildcards::one_agent("acp", "s1"),
            "acp.session.s1.agent.>"
        );
    }

    #[test]
    fn session_wildcard_one_client() {
        assert_eq!(
            session::wildcards::one_client("acp", "s1"),
            "acp.session.s1.client.>"
        );
    }

    #[test]
    fn custom_prefix_global() {
        assert_eq!(agent::initialize("myapp"), "myapp.agent.initialize");
        assert_eq!(agent::session_new("myapp"), "myapp.agent.session.new");
    }

    #[test]
    fn custom_prefix_session() {
        assert_eq!(
            session::agent::prompt("myapp", "s1"),
            "myapp.session.s1.agent.prompt"
        );
        assert_eq!(
            session::client::fs_read_text_file("myapp", "s1"),
            "myapp.session.s1.client.fs.read_text_file"
        );
    }

    #[test]
    fn no_overlap_agent_and_session_are_distinct() {
        let global = agent::wildcards::all("acp");
        let session_agent = session::wildcards::all_agent("acp");

        assert!(global.starts_with("acp.agent."));
        assert!(session_agent.starts_with("acp.session."));
    }

    #[test]
    fn session_scoped_agent_subjects_share_layout() {
        let prefix = "acp";
        let sid = "abc";
        let expected = format!("{}.session.{}.agent.", prefix, sid);

        assert!(session::agent::load(prefix, sid).starts_with(&expected));
        assert!(session::agent::prompt(prefix, sid).starts_with(&expected));
        assert!(session::agent::cancel(prefix, sid).starts_with(&expected));
        assert!(session::agent::set_mode(prefix, sid).starts_with(&expected));
        assert!(session::agent::set_config_option(prefix, sid).starts_with(&expected));
        assert!(session::agent::set_model(prefix, sid).starts_with(&expected));
        assert!(session::agent::fork(prefix, sid).starts_with(&expected));
        assert!(session::agent::resume(prefix, sid).starts_with(&expected));
        assert!(session::agent::close(prefix, sid).starts_with(&expected));
    }

    #[test]
    fn session_scoped_client_subjects_share_layout() {
        let prefix = "acp";
        let sid = "abc";
        let expected = format!("{}.session.{}.client.", prefix, sid);

        assert!(session::client::fs_read_text_file(prefix, sid).starts_with(&expected));
        assert!(session::client::fs_write_text_file(prefix, sid).starts_with(&expected));
        assert!(session::client::session_request_permission(prefix, sid).starts_with(&expected));
        assert!(session::client::session_update(prefix, sid).starts_with(&expected));
        assert!(session::client::terminal_create(prefix, sid).starts_with(&expected));
        assert!(session::client::terminal_kill(prefix, sid).starts_with(&expected));
        assert!(session::client::terminal_output(prefix, sid).starts_with(&expected));
        assert!(session::client::terminal_release(prefix, sid).starts_with(&expected));
        assert!(session::client::terminal_wait_for_exit(prefix, sid).starts_with(&expected));
    }
}
