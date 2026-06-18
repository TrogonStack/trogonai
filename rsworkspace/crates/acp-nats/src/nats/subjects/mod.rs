pub mod client_ops;
pub mod commands;
pub mod global;
pub mod markers;
pub mod responses;
pub mod stream;
pub mod subscriptions;

pub use stream::{AcpStream, StreamAssignment};

pub mod agent {
    pub use super::global::{
        AuthenticateSubject, ExtNotifySubject, ExtSubject, InitializeSubject, LogoutSubject, SessionListSubject,
        SessionNewSubject,
    };

    pub mod wildcards {
        pub use super::super::subscriptions::GlobalAllSubject;
    }
}

pub mod session {
    pub mod agent {
        pub use super::super::commands::{
            CancelSubject, CloseSubject, ForkSubject, LoadSubject, PromptSubject, ResumeSubject,
            SetConfigOptionSubject, SetModeSubject, SetModelSubject,
        };
        pub use super::super::responses::{
            CancelledSubject, ExtReadySubject, PromptResponseSubject, ResponseSubject, UpdateSubject,
        };
        pub use super::super::subscriptions::PromptWildcardSubject;
    }

    pub mod client {
        pub use super::super::client_ops::{
            FsReadTextFileSubject, FsWriteTextFileSubject, SessionRequestPermissionSubject, SessionUpdateSubject,
            TerminalCreateSubject, TerminalKillSubject, TerminalOutputSubject, TerminalReleaseSubject,
            TerminalWaitForExitSubject,
        };
    }

    pub mod wildcards {
        pub use super::super::subscriptions::{
            AllAgentExtSubject, AllAgentSubject, AllClientSubject, AllSessionSubject, OneAgentSubject,
            OneClientSubject, OneSessionSubject,
        };
    }
}

#[cfg(test)]
mod tests {
    use super::{AcpStream, StreamAssignment, agent, session};
    use crate::acp_prefix::AcpPrefix;
    use crate::ext_method_name::ExtMethodName;
    use crate::req_id::ReqId;
    use crate::session_id::AcpSessionId;

    fn p(s: &str) -> AcpPrefix {
        AcpPrefix::new(s).expect("test prefix")
    }

    fn sid(s: &str) -> AcpSessionId {
        AcpSessionId::new(s).expect("test session id")
    }

    fn method(s: &str) -> ExtMethodName {
        ExtMethodName::new(s).expect("test method name")
    }

    fn rid(s: &str) -> ReqId {
        ReqId::from_test(s)
    }

    #[test]
    fn agent_initialize() {
        assert_eq!(
            agent::InitializeSubject::new(&p("acp")).to_string(),
            "acp.agent.initialize"
        );
    }

    #[test]
    fn agent_authenticate() {
        assert_eq!(
            agent::AuthenticateSubject::new(&p("acp")).to_string(),
            "acp.agent.authenticate"
        );
    }

    #[test]
    fn agent_logout() {
        assert_eq!(agent::LogoutSubject::new(&p("acp")).to_string(), "acp.agent.logout");
    }

    #[test]
    fn agent_session_new() {
        assert_eq!(
            agent::SessionNewSubject::new(&p("acp")).to_string(),
            "acp.agent.session.new"
        );
    }

    #[test]
    fn agent_session_list() {
        assert_eq!(
            agent::SessionListSubject::new(&p("acp")).to_string(),
            "acp.agent.session.list"
        );
    }

    #[test]
    fn agent_ext() {
        assert_eq!(
            agent::ExtSubject::new(&p("acp"), &method("my_tool")).to_string(),
            "acp.agent.ext.my_tool"
        );
    }

    #[test]
    fn agent_ext_dotted() {
        assert_eq!(
            agent::ExtSubject::new(&p("acp"), &method("vendor.op")).to_string(),
            "acp.agent.ext.vendor.op"
        );
    }

    #[test]
    fn agent_wildcard_all() {
        assert_eq!(
            agent::wildcards::GlobalAllSubject::new(&p("acp")).to_string(),
            "acp.agent.>"
        );
    }

    #[test]
    fn session_agent_load() {
        assert_eq!(
            session::agent::LoadSubject::new(&p("acp"), &sid("s1")).to_string(),
            "acp.session.s1.agent.load"
        );
    }

    #[test]
    fn session_agent_prompt() {
        assert_eq!(
            session::agent::PromptSubject::new(&p("acp"), &sid("s1")).to_string(),
            "acp.session.s1.agent.prompt"
        );
    }

    #[test]
    fn session_agent_prompt_wildcard() {
        assert_eq!(
            session::agent::PromptWildcardSubject::new(&p("acp")).to_string(),
            "acp.session.*.agent.prompt"
        );
    }

    #[test]
    fn session_agent_cancel() {
        assert_eq!(
            session::agent::CancelSubject::new(&p("acp"), &sid("s1")).to_string(),
            "acp.session.s1.agent.cancel"
        );
    }

    #[test]
    fn session_agent_cancelled() {
        assert_eq!(
            session::agent::CancelledSubject::new(&p("acp"), &sid("s1")).to_string(),
            "acp.session.s1.agent.cancelled"
        );
    }

    #[test]
    fn session_agent_set_mode() {
        assert_eq!(
            session::agent::SetModeSubject::new(&p("acp"), &sid("s1")).to_string(),
            "acp.session.s1.agent.set_mode"
        );
    }

    #[test]
    fn session_agent_set_config_option() {
        assert_eq!(
            session::agent::SetConfigOptionSubject::new(&p("acp"), &sid("s1")).to_string(),
            "acp.session.s1.agent.set_config_option"
        );
    }

    #[test]
    fn session_agent_set_model() {
        assert_eq!(
            session::agent::SetModelSubject::new(&p("acp"), &sid("s1")).to_string(),
            "acp.session.s1.agent.set_model"
        );
    }

    #[test]
    fn session_agent_fork() {
        assert_eq!(
            session::agent::ForkSubject::new(&p("acp"), &sid("s1")).to_string(),
            "acp.session.s1.agent.fork"
        );
    }

    #[test]
    fn session_agent_resume() {
        assert_eq!(
            session::agent::ResumeSubject::new(&p("acp"), &sid("s1")).to_string(),
            "acp.session.s1.agent.resume"
        );
    }

    #[test]
    fn session_agent_close() {
        assert_eq!(
            session::agent::CloseSubject::new(&p("acp"), &sid("s1")).to_string(),
            "acp.session.s1.agent.close"
        );
    }

    #[test]
    fn session_agent_ext_ready() {
        assert_eq!(
            session::agent::ExtReadySubject::new(&p("acp"), &sid("s1")).to_string(),
            "acp.session.s1.agent.ext.ready"
        );
    }

    #[test]
    fn session_agent_update() {
        assert_eq!(
            session::agent::UpdateSubject::new(&p("acp"), &sid("s1"), &rid("req-abc")).to_string(),
            "acp.session.s1.agent.update.req-abc"
        );
    }

    #[test]
    fn session_agent_prompt_response() {
        assert_eq!(
            session::agent::PromptResponseSubject::new(&p("acp"), &sid("s1"), &rid("req-abc")).to_string(),
            "acp.session.s1.agent.prompt.response.req-abc"
        );
    }

    #[test]
    fn session_agent_response() {
        assert_eq!(
            session::agent::ResponseSubject::new(&p("acp"), &sid("s1"), &rid("req-abc")).to_string(),
            "acp.session.s1.agent.response.req-abc"
        );
    }

    #[test]
    fn session_client_fs_read() {
        assert_eq!(
            session::client::FsReadTextFileSubject::new(&p("acp"), &sid("s1")).to_string(),
            "acp.session.s1.client.fs.read_text_file"
        );
    }

    #[test]
    fn session_client_fs_write() {
        assert_eq!(
            session::client::FsWriteTextFileSubject::new(&p("acp"), &sid("s1")).to_string(),
            "acp.session.s1.client.fs.write_text_file"
        );
    }

    #[test]
    fn session_client_request_permission() {
        assert_eq!(
            session::client::SessionRequestPermissionSubject::new(&p("acp"), &sid("s1")).to_string(),
            "acp.session.s1.client.session.request_permission"
        );
    }

    #[test]
    fn session_client_session_update() {
        assert_eq!(
            session::client::SessionUpdateSubject::new(&p("acp"), &sid("s1")).to_string(),
            "acp.session.s1.client.session.update"
        );
    }

    #[test]
    fn session_client_terminal_create() {
        assert_eq!(
            session::client::TerminalCreateSubject::new(&p("acp"), &sid("s1")).to_string(),
            "acp.session.s1.client.terminal.create"
        );
    }

    #[test]
    fn session_client_terminal_kill() {
        assert_eq!(
            session::client::TerminalKillSubject::new(&p("acp"), &sid("s1")).to_string(),
            "acp.session.s1.client.terminal.kill"
        );
    }

    #[test]
    fn session_client_terminal_output() {
        assert_eq!(
            session::client::TerminalOutputSubject::new(&p("acp"), &sid("s1")).to_string(),
            "acp.session.s1.client.terminal.output"
        );
    }

    #[test]
    fn session_client_terminal_release() {
        assert_eq!(
            session::client::TerminalReleaseSubject::new(&p("acp"), &sid("s1")).to_string(),
            "acp.session.s1.client.terminal.release"
        );
    }

    #[test]
    fn session_client_terminal_wait_for_exit() {
        assert_eq!(
            session::client::TerminalWaitForExitSubject::new(&p("acp"), &sid("s1")).to_string(),
            "acp.session.s1.client.terminal.wait_for_exit"
        );
    }

    #[test]
    fn session_wildcard_all() {
        assert_eq!(
            session::wildcards::AllSessionSubject::new(&p("acp")).to_string(),
            "acp.session.>"
        );
    }

    #[test]
    fn session_wildcard_one() {
        assert_eq!(
            session::wildcards::OneSessionSubject::new(&p("acp"), &sid("s1")).to_string(),
            "acp.session.s1.>"
        );
    }

    #[test]
    fn session_wildcard_all_agent() {
        assert_eq!(
            session::wildcards::AllAgentSubject::new(&p("acp")).to_string(),
            "acp.session.*.agent.>"
        );
    }

    #[test]
    fn session_wildcard_all_client() {
        assert_eq!(
            session::wildcards::AllClientSubject::new(&p("acp")).to_string(),
            "acp.session.*.client.>"
        );
    }

    #[test]
    fn session_wildcard_one_agent() {
        assert_eq!(
            session::wildcards::OneAgentSubject::new(&p("acp"), &sid("s1")).to_string(),
            "acp.session.s1.agent.>"
        );
    }

    #[test]
    fn session_wildcard_one_client() {
        assert_eq!(
            session::wildcards::OneClientSubject::new(&p("acp"), &sid("s1")).to_string(),
            "acp.session.s1.client.>"
        );
    }

    #[test]
    fn custom_prefix_global() {
        assert_eq!(
            agent::InitializeSubject::new(&p("myapp")).to_string(),
            "myapp.agent.initialize"
        );
        assert_eq!(
            agent::SessionNewSubject::new(&p("myapp")).to_string(),
            "myapp.agent.session.new"
        );
    }

    #[test]
    fn custom_prefix_session() {
        assert_eq!(
            session::agent::PromptSubject::new(&p("myapp"), &sid("s1")).to_string(),
            "myapp.session.s1.agent.prompt"
        );
        assert_eq!(
            session::client::FsReadTextFileSubject::new(&p("myapp"), &sid("s1")).to_string(),
            "myapp.session.s1.client.fs.read_text_file"
        );
    }

    #[test]
    fn stream_assignments() {
        assert_eq!(session::agent::LoadSubject::STREAM, Some(AcpStream::Commands));
        assert_eq!(session::agent::PromptSubject::STREAM, Some(AcpStream::Commands));
        assert_eq!(session::agent::CancelSubject::STREAM, Some(AcpStream::Commands));
        assert_eq!(session::agent::CloseSubject::STREAM, Some(AcpStream::Commands));
        assert_eq!(session::agent::ForkSubject::STREAM, Some(AcpStream::Commands));
        assert_eq!(session::agent::ResumeSubject::STREAM, Some(AcpStream::Commands));
        assert_eq!(session::agent::SetModeSubject::STREAM, Some(AcpStream::Commands));
        assert_eq!(
            session::agent::SetConfigOptionSubject::STREAM,
            Some(AcpStream::Commands)
        );
        assert_eq!(session::agent::SetModelSubject::STREAM, Some(AcpStream::Commands));

        assert_eq!(agent::InitializeSubject::STREAM, Some(AcpStream::Global));
        assert_eq!(agent::AuthenticateSubject::STREAM, Some(AcpStream::Global));
        assert_eq!(agent::LogoutSubject::STREAM, Some(AcpStream::Global));
        assert_eq!(agent::SessionNewSubject::STREAM, Some(AcpStream::Global));
        assert_eq!(agent::SessionListSubject::STREAM, None);
        assert_eq!(agent::ExtSubject::STREAM, Some(AcpStream::GlobalExt));
        assert_eq!(agent::ExtNotifySubject::STREAM, Some(AcpStream::GlobalExt));

        assert_eq!(session::agent::CancelledSubject::STREAM, Some(AcpStream::Responses));
        assert_eq!(session::agent::ExtReadySubject::STREAM, Some(AcpStream::Responses));
        assert_eq!(
            session::agent::PromptResponseSubject::STREAM,
            Some(AcpStream::Responses)
        );
        assert_eq!(session::agent::ResponseSubject::STREAM, Some(AcpStream::Responses));
        assert_eq!(session::agent::UpdateSubject::STREAM, Some(AcpStream::Notifications));

        assert_eq!(
            session::client::FsReadTextFileSubject::STREAM,
            Some(AcpStream::ClientOps)
        );
        assert_eq!(
            session::client::FsWriteTextFileSubject::STREAM,
            Some(AcpStream::ClientOps)
        );
        assert_eq!(
            session::client::SessionRequestPermissionSubject::STREAM,
            Some(AcpStream::ClientOps)
        );
        assert_eq!(
            session::client::SessionUpdateSubject::STREAM,
            Some(AcpStream::ClientOps)
        );
        assert_eq!(
            session::client::TerminalCreateSubject::STREAM,
            Some(AcpStream::ClientOps)
        );
        assert_eq!(session::client::TerminalKillSubject::STREAM, Some(AcpStream::ClientOps));
        assert_eq!(
            session::client::TerminalOutputSubject::STREAM,
            Some(AcpStream::ClientOps)
        );
        assert_eq!(
            session::client::TerminalReleaseSubject::STREAM,
            Some(AcpStream::ClientOps)
        );
        assert_eq!(
            session::client::TerminalWaitForExitSubject::STREAM,
            Some(AcpStream::ClientOps)
        );

        assert_eq!(session::wildcards::AllAgentSubject::STREAM, None);
        assert_eq!(session::wildcards::AllAgentExtSubject::STREAM, None);
        assert_eq!(session::wildcards::AllClientSubject::STREAM, None);
        assert_eq!(session::wildcards::AllSessionSubject::STREAM, None);
        assert_eq!(agent::wildcards::GlobalAllSubject::STREAM, None);
        assert_eq!(session::wildcards::OneAgentSubject::STREAM, None);
        assert_eq!(session::wildcards::OneClientSubject::STREAM, None);
        assert_eq!(session::wildcards::OneSessionSubject::STREAM, None);
        assert_eq!(session::agent::PromptWildcardSubject::STREAM, None);
    }

    #[test]
    fn acp_stream_display() {
        assert_eq!(AcpStream::Commands.to_string(), "COMMANDS");
        assert_eq!(AcpStream::Responses.to_string(), "RESPONSES");
        assert_eq!(AcpStream::ClientOps.to_string(), "CLIENT_OPS");
        assert_eq!(AcpStream::Notifications.to_string(), "NOTIFICATIONS");
        assert_eq!(AcpStream::Global.to_string(), "GLOBAL");
        assert_eq!(AcpStream::GlobalExt.to_string(), "GLOBAL_EXT");
    }

    #[test]
    fn no_overlap_agent_and_session_are_distinct() {
        let global = agent::wildcards::GlobalAllSubject::new(&p("acp"));
        let session_agent = session::wildcards::AllAgentSubject::new(&p("acp"));

        assert!(global.to_string().starts_with("acp.agent."));
        assert!(session_agent.to_string().starts_with("acp.session."));
    }

    #[test]
    fn session_scoped_agent_subjects_share_layout() {
        let prefix = p("acp");
        let session_id = sid("abc");
        let expected = format!("{}.session.{}.agent.", prefix.as_str(), session_id.as_str());

        assert!(
            session::agent::LoadSubject::new(&prefix, &session_id)
                .to_string()
                .starts_with(&expected)
        );
        assert!(
            session::agent::PromptSubject::new(&prefix, &session_id)
                .to_string()
                .starts_with(&expected)
        );
        assert!(
            session::agent::CancelSubject::new(&prefix, &session_id)
                .to_string()
                .starts_with(&expected)
        );
        assert!(
            session::agent::SetModeSubject::new(&prefix, &session_id)
                .to_string()
                .starts_with(&expected)
        );
        assert!(
            session::agent::SetConfigOptionSubject::new(&prefix, &session_id)
                .to_string()
                .starts_with(&expected)
        );
        assert!(
            session::agent::SetModelSubject::new(&prefix, &session_id)
                .to_string()
                .starts_with(&expected)
        );
        assert!(
            session::agent::ForkSubject::new(&prefix, &session_id)
                .to_string()
                .starts_with(&expected)
        );
        assert!(
            session::agent::ResumeSubject::new(&prefix, &session_id)
                .to_string()
                .starts_with(&expected)
        );
        assert!(
            session::agent::CloseSubject::new(&prefix, &session_id)
                .to_string()
                .starts_with(&expected)
        );
    }

    #[test]
    fn session_scoped_client_subjects_share_layout() {
        let prefix = p("acp");
        let session_id = sid("abc");
        let expected = format!("{}.session.{}.client.", prefix.as_str(), session_id.as_str());

        assert!(
            session::client::FsReadTextFileSubject::new(&prefix, &session_id)
                .to_string()
                .starts_with(&expected)
        );
        assert!(
            session::client::FsWriteTextFileSubject::new(&prefix, &session_id)
                .to_string()
                .starts_with(&expected)
        );
        assert!(
            session::client::SessionRequestPermissionSubject::new(&prefix, &session_id)
                .to_string()
                .starts_with(&expected)
        );
        assert!(
            session::client::SessionUpdateSubject::new(&prefix, &session_id)
                .to_string()
                .starts_with(&expected)
        );
        assert!(
            session::client::TerminalCreateSubject::new(&prefix, &session_id)
                .to_string()
                .starts_with(&expected)
        );
        assert!(
            session::client::TerminalKillSubject::new(&prefix, &session_id)
                .to_string()
                .starts_with(&expected)
        );
        assert!(
            session::client::TerminalOutputSubject::new(&prefix, &session_id)
                .to_string()
                .starts_with(&expected)
        );
        assert!(
            session::client::TerminalReleaseSubject::new(&prefix, &session_id)
                .to_string()
                .starts_with(&expected)
        );
        assert!(
            session::client::TerminalWaitForExitSubject::new(&prefix, &session_id)
                .to_string()
                .starts_with(&expected)
        );
    }

    #[test]
    fn to_subject_produces_correct_nats_subject() {
        use async_nats::subject::ToSubject;

        let prefix = p("acp");
        let sid = sid("s1");

        assert_eq!(
            session::agent::LoadSubject::new(&prefix, &sid).to_subject().as_str(),
            "acp.session.s1.agent.load"
        );
        assert_eq!(
            session::agent::CancelSubject::new(&prefix, &sid).to_subject().as_str(),
            "acp.session.s1.agent.cancel"
        );
        assert_eq!(
            session::agent::CloseSubject::new(&prefix, &sid).to_subject().as_str(),
            "acp.session.s1.agent.close"
        );
        assert_eq!(
            session::agent::ForkSubject::new(&prefix, &sid).to_subject().as_str(),
            "acp.session.s1.agent.fork"
        );
        assert_eq!(
            session::agent::ResumeSubject::new(&prefix, &sid).to_subject().as_str(),
            "acp.session.s1.agent.resume"
        );
        assert_eq!(
            session::agent::SetModeSubject::new(&prefix, &sid).to_subject().as_str(),
            "acp.session.s1.agent.set_mode"
        );
        assert_eq!(
            session::agent::SetConfigOptionSubject::new(&prefix, &sid)
                .to_subject()
                .as_str(),
            "acp.session.s1.agent.set_config_option"
        );
        assert_eq!(
            session::agent::SetModelSubject::new(&prefix, &sid)
                .to_subject()
                .as_str(),
            "acp.session.s1.agent.set_model"
        );
        // ExtNotifySubject doesn't impl ToSubject — verify via Display
        assert_eq!(
            agent::ExtNotifySubject::new(&prefix, &method("my_tool")).to_string(),
            "acp.agent.ext.my_tool"
        );

        // Subscription subjects
        assert_eq!(
            session::wildcards::AllSessionSubject::new(&prefix)
                .to_subject()
                .as_str(),
            "acp.session.>"
        );
        assert_eq!(
            session::wildcards::OneSessionSubject::new(&prefix, &sid)
                .to_subject()
                .as_str(),
            "acp.session.s1.>"
        );
        assert_eq!(
            session::wildcards::OneAgentSubject::new(&prefix, &sid)
                .to_subject()
                .as_str(),
            "acp.session.s1.agent.>"
        );
        assert_eq!(
            session::wildcards::OneClientSubject::new(&prefix, &sid)
                .to_subject()
                .as_str(),
            "acp.session.s1.client.>"
        );
        assert_eq!(
            session::agent::PromptWildcardSubject::new(&prefix)
                .to_subject()
                .as_str(),
            "acp.session.*.agent.prompt"
        );

        // Response subject
        assert_eq!(
            session::agent::ResponseSubject::new(&prefix, &sid, &rid("req-abc"))
                .to_subject()
                .as_str(),
            "acp.session.s1.agent.response.req-abc"
        );
    }
}
