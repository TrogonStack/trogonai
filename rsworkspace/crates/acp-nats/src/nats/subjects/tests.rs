use super::{AcpStream, StreamAssignment, client_ops, commands, global, responses, subscriptions};
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
            global::InitializeSubject::new(&p("acp")).to_string(),
            "acp.agent.initialize"
        );
    }

    #[test]
    fn agent_authenticate() {
        assert_eq!(
            global::AuthenticateSubject::new(&p("acp")).to_string(),
            "acp.agent.authenticate"
        );
    }

    #[test]
    fn agent_logout() {
        assert_eq!(global::LogoutSubject::new(&p("acp")).to_string(), "acp.agent.logout");
    }

    #[test]
    fn agent_session_new() {
        assert_eq!(
            global::SessionNewSubject::new(&p("acp")).to_string(),
            "acp.agent.session.new"
        );
    }

    #[test]
    fn agent_session_list() {
        assert_eq!(
            global::SessionListSubject::new(&p("acp")).to_string(),
            "acp.agent.session.list"
        );
    }

    #[test]
    fn agent_ext() {
        assert_eq!(
            global::ExtSubject::new(&p("acp"), &method("my_tool")).to_string(),
            "acp.agent.ext.my_tool"
        );
    }

    #[test]
    fn agent_ext_dotted() {
        assert_eq!(
            global::ExtSubject::new(&p("acp"), &method("vendor.op")).to_string(),
            "acp.agent.ext.vendor.op"
        );
    }

    #[test]
    fn agent_wildcard_all() {
        assert_eq!(
            subscriptions::GlobalAllSubject::new(&p("acp")).to_string(),
            "acp.agent.>"
        );
    }

    #[test]
    fn session_agent_load() {
        assert_eq!(
            commands::LoadSubject::new(&p("acp"), &sid("s1")).to_string(),
            "acp.session.s1.agent.load"
        );
    }

    #[test]
    fn session_agent_prompt() {
        assert_eq!(
            commands::PromptSubject::new(&p("acp"), &sid("s1")).to_string(),
            "acp.session.s1.agent.prompt"
        );
    }

    #[test]
    fn session_agent_prompt_wildcard() {
        assert_eq!(
            subscriptions::PromptWildcardSubject::new(&p("acp")).to_string(),
            "acp.session.*.agent.prompt"
        );
    }

    #[test]
    fn session_agent_cancel() {
        assert_eq!(
            commands::CancelSubject::new(&p("acp"), &sid("s1")).to_string(),
            "acp.session.s1.agent.cancel"
        );
    }

    #[test]
    fn session_agent_cancelled() {
        assert_eq!(
            responses::CancelledSubject::new(&p("acp"), &sid("s1")).to_string(),
            "acp.session.s1.agent.cancelled"
        );
    }

    #[test]
    fn session_agent_set_mode() {
        assert_eq!(
            commands::SetModeSubject::new(&p("acp"), &sid("s1")).to_string(),
            "acp.session.s1.agent.set_mode"
        );
    }

    #[test]
    fn session_agent_set_config_option() {
        assert_eq!(
            commands::SetConfigOptionSubject::new(&p("acp"), &sid("s1")).to_string(),
            "acp.session.s1.agent.set_config_option"
        );
    }

    #[test]
    fn session_agent_set_model() {
        assert_eq!(
            commands::SetModelSubject::new(&p("acp"), &sid("s1")).to_string(),
            "acp.session.s1.agent.set_model"
        );
    }

    #[test]
    fn session_agent_fork() {
        assert_eq!(
            commands::ForkSubject::new(&p("acp"), &sid("s1")).to_string(),
            "acp.session.s1.agent.fork"
        );
    }

    #[test]
    fn session_agent_resume() {
        assert_eq!(
            commands::ResumeSubject::new(&p("acp"), &sid("s1")).to_string(),
            "acp.session.s1.agent.resume"
        );
    }

    #[test]
    fn session_agent_close() {
        assert_eq!(
            commands::CloseSubject::new(&p("acp"), &sid("s1")).to_string(),
            "acp.session.s1.agent.close"
        );
    }

    #[test]
    fn session_agent_ext_ready() {
        assert_eq!(
            responses::ExtReadySubject::new(&p("acp"), &sid("s1")).to_string(),
            "acp.session.s1.agent.ext.ready"
        );
    }

    #[test]
    fn session_agent_update() {
        assert_eq!(
            responses::UpdateSubject::new(&p("acp"), &sid("s1"), &rid("req-abc")).to_string(),
            "acp.session.s1.agent.update.req-abc"
        );
    }

    #[test]
    fn session_agent_prompt_response() {
        assert_eq!(
            responses::PromptResponseSubject::new(&p("acp"), &sid("s1"), &rid("req-abc")).to_string(),
            "acp.session.s1.agent.prompt.response.req-abc"
        );
    }

    #[test]
    fn session_agent_response() {
        assert_eq!(
            responses::ResponseSubject::new(&p("acp"), &sid("s1"), &rid("req-abc")).to_string(),
            "acp.session.s1.agent.response.req-abc"
        );
    }

    #[test]
    fn session_client_fs_read() {
        assert_eq!(
            client_ops::FsReadTextFileSubject::new(&p("acp"), &sid("s1")).to_string(),
            "acp.session.s1.client.fs.read_text_file"
        );
    }

    #[test]
    fn session_client_fs_write() {
        assert_eq!(
            client_ops::FsWriteTextFileSubject::new(&p("acp"), &sid("s1")).to_string(),
            "acp.session.s1.client.fs.write_text_file"
        );
    }

    #[test]
    fn session_client_request_permission() {
        assert_eq!(
            client_ops::SessionRequestPermissionSubject::new(&p("acp"), &sid("s1")).to_string(),
            "acp.session.s1.client.session.request_permission"
        );
    }

    #[test]
    fn session_client_session_update() {
        assert_eq!(
            client_ops::SessionUpdateSubject::new(&p("acp"), &sid("s1")).to_string(),
            "acp.session.s1.client.session.update"
        );
    }

    #[test]
    fn session_client_terminal_create() {
        assert_eq!(
            client_ops::TerminalCreateSubject::new(&p("acp"), &sid("s1")).to_string(),
            "acp.session.s1.client.terminal.create"
        );
    }

    #[test]
    fn session_client_terminal_kill() {
        assert_eq!(
            client_ops::TerminalKillSubject::new(&p("acp"), &sid("s1")).to_string(),
            "acp.session.s1.client.terminal.kill"
        );
    }

    #[test]
    fn session_client_terminal_output() {
        assert_eq!(
            client_ops::TerminalOutputSubject::new(&p("acp"), &sid("s1")).to_string(),
            "acp.session.s1.client.terminal.output"
        );
    }

    #[test]
    fn session_client_terminal_release() {
        assert_eq!(
            client_ops::TerminalReleaseSubject::new(&p("acp"), &sid("s1")).to_string(),
            "acp.session.s1.client.terminal.release"
        );
    }

    #[test]
    fn session_client_terminal_wait_for_exit() {
        assert_eq!(
            client_ops::TerminalWaitForExitSubject::new(&p("acp"), &sid("s1")).to_string(),
            "acp.session.s1.client.terminal.wait_for_exit"
        );
    }

    #[test]
    fn session_wildcard_all() {
        assert_eq!(
            subscriptions::AllSessionSubject::new(&p("acp")).to_string(),
            "acp.session.>"
        );
    }

    #[test]
    fn session_wildcard_one() {
        assert_eq!(
            subscriptions::OneSessionSubject::new(&p("acp"), &sid("s1")).to_string(),
            "acp.session.s1.>"
        );
    }

    #[test]
    fn session_wildcard_all_agent() {
        assert_eq!(
            subscriptions::AllAgentSubject::new(&p("acp")).to_string(),
            "acp.session.*.agent.>"
        );
    }

    #[test]
    fn session_wildcard_all_client() {
        assert_eq!(
            subscriptions::AllClientSubject::new(&p("acp")).to_string(),
            "acp.session.*.client.>"
        );
    }

    #[test]
    fn session_wildcard_one_agent() {
        assert_eq!(
            subscriptions::OneAgentSubject::new(&p("acp"), &sid("s1")).to_string(),
            "acp.session.s1.agent.>"
        );
    }

    #[test]
    fn session_wildcard_one_client() {
        assert_eq!(
            subscriptions::OneClientSubject::new(&p("acp"), &sid("s1")).to_string(),
            "acp.session.s1.client.>"
        );
    }

    #[test]
    fn custom_prefix_global() {
        assert_eq!(
            global::InitializeSubject::new(&p("myapp")).to_string(),
            "myapp.agent.initialize"
        );
        assert_eq!(
            global::SessionNewSubject::new(&p("myapp")).to_string(),
            "myapp.agent.session.new"
        );
    }

    #[test]
    fn custom_prefix_session() {
        assert_eq!(
            commands::PromptSubject::new(&p("myapp"), &sid("s1")).to_string(),
            "myapp.session.s1.agent.prompt"
        );
        assert_eq!(
            client_ops::FsReadTextFileSubject::new(&p("myapp"), &sid("s1")).to_string(),
            "myapp.session.s1.client.fs.read_text_file"
        );
    }

    #[test]
    fn stream_assignments() {
        assert_eq!(commands::LoadSubject::STREAM, Some(AcpStream::Commands));
        assert_eq!(commands::PromptSubject::STREAM, Some(AcpStream::Commands));
        assert_eq!(commands::CancelSubject::STREAM, Some(AcpStream::Commands));
        assert_eq!(commands::CloseSubject::STREAM, Some(AcpStream::Commands));
        assert_eq!(commands::ForkSubject::STREAM, Some(AcpStream::Commands));
        assert_eq!(commands::ResumeSubject::STREAM, Some(AcpStream::Commands));
        assert_eq!(commands::SetModeSubject::STREAM, Some(AcpStream::Commands));
        assert_eq!(commands::SetConfigOptionSubject::STREAM, Some(AcpStream::Commands));
        assert_eq!(commands::SetModelSubject::STREAM, Some(AcpStream::Commands));

        assert_eq!(global::InitializeSubject::STREAM, Some(AcpStream::Global));
        assert_eq!(global::AuthenticateSubject::STREAM, Some(AcpStream::Global));
        assert_eq!(global::LogoutSubject::STREAM, Some(AcpStream::Global));
        assert_eq!(global::SessionNewSubject::STREAM, Some(AcpStream::Global));
        assert_eq!(global::SessionListSubject::STREAM, None);
        assert_eq!(global::ExtSubject::STREAM, Some(AcpStream::GlobalExt));
        assert_eq!(global::ExtNotifySubject::STREAM, Some(AcpStream::GlobalExt));

        assert_eq!(responses::CancelledSubject::STREAM, Some(AcpStream::Responses));
        assert_eq!(responses::ExtReadySubject::STREAM, Some(AcpStream::Responses));
        assert_eq!(responses::PromptResponseSubject::STREAM, Some(AcpStream::Responses));
        assert_eq!(responses::ResponseSubject::STREAM, Some(AcpStream::Responses));
        assert_eq!(responses::UpdateSubject::STREAM, Some(AcpStream::Notifications));

        assert_eq!(client_ops::FsReadTextFileSubject::STREAM, Some(AcpStream::ClientOps));
        assert_eq!(client_ops::FsWriteTextFileSubject::STREAM, Some(AcpStream::ClientOps));
        assert_eq!(
            client_ops::SessionRequestPermissionSubject::STREAM,
            Some(AcpStream::ClientOps)
        );
        assert_eq!(client_ops::SessionUpdateSubject::STREAM, Some(AcpStream::ClientOps));
        assert_eq!(client_ops::TerminalCreateSubject::STREAM, Some(AcpStream::ClientOps));
        assert_eq!(client_ops::TerminalKillSubject::STREAM, Some(AcpStream::ClientOps));
        assert_eq!(client_ops::TerminalOutputSubject::STREAM, Some(AcpStream::ClientOps));
        assert_eq!(client_ops::TerminalReleaseSubject::STREAM, Some(AcpStream::ClientOps));
        assert_eq!(
            client_ops::TerminalWaitForExitSubject::STREAM,
            Some(AcpStream::ClientOps)
        );

        assert_eq!(subscriptions::AllAgentSubject::STREAM, None);
        assert_eq!(subscriptions::AllAgentExtSubject::STREAM, None);
        assert_eq!(subscriptions::AllClientSubject::STREAM, None);
        assert_eq!(subscriptions::AllSessionSubject::STREAM, None);
        assert_eq!(subscriptions::GlobalAllSubject::STREAM, None);
        assert_eq!(subscriptions::OneAgentSubject::STREAM, None);
        assert_eq!(subscriptions::OneClientSubject::STREAM, None);
        assert_eq!(subscriptions::OneSessionSubject::STREAM, None);
        assert_eq!(subscriptions::PromptWildcardSubject::STREAM, None);
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
        let global = subscriptions::GlobalAllSubject::new(&p("acp"));
        let session_agent = subscriptions::AllAgentSubject::new(&p("acp"));

        assert!(global.to_string().starts_with("acp.agent."));
        assert!(session_agent.to_string().starts_with("acp.session."));
    }

    #[test]
    fn session_scoped_agent_subjects_share_layout() {
        let prefix = p("acp");
        let session_id = sid("abc");
        let expected = format!("{}.session.{}.agent.", prefix.as_str(), session_id.as_str());

        assert!(
            commands::LoadSubject::new(&prefix, &session_id)
                .to_string()
                .starts_with(&expected)
        );
        assert!(
            commands::PromptSubject::new(&prefix, &session_id)
                .to_string()
                .starts_with(&expected)
        );
        assert!(
            commands::CancelSubject::new(&prefix, &session_id)
                .to_string()
                .starts_with(&expected)
        );
        assert!(
            commands::SetModeSubject::new(&prefix, &session_id)
                .to_string()
                .starts_with(&expected)
        );
        assert!(
            commands::SetConfigOptionSubject::new(&prefix, &session_id)
                .to_string()
                .starts_with(&expected)
        );
        assert!(
            commands::SetModelSubject::new(&prefix, &session_id)
                .to_string()
                .starts_with(&expected)
        );
        assert!(
            commands::ForkSubject::new(&prefix, &session_id)
                .to_string()
                .starts_with(&expected)
        );
        assert!(
            commands::ResumeSubject::new(&prefix, &session_id)
                .to_string()
                .starts_with(&expected)
        );
        assert!(
            commands::CloseSubject::new(&prefix, &session_id)
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
            client_ops::FsReadTextFileSubject::new(&prefix, &session_id)
                .to_string()
                .starts_with(&expected)
        );
        assert!(
            client_ops::FsWriteTextFileSubject::new(&prefix, &session_id)
                .to_string()
                .starts_with(&expected)
        );
        assert!(
            client_ops::SessionRequestPermissionSubject::new(&prefix, &session_id)
                .to_string()
                .starts_with(&expected)
        );
        assert!(
            client_ops::SessionUpdateSubject::new(&prefix, &session_id)
                .to_string()
                .starts_with(&expected)
        );
        assert!(
            client_ops::TerminalCreateSubject::new(&prefix, &session_id)
                .to_string()
                .starts_with(&expected)
        );
        assert!(
            client_ops::TerminalKillSubject::new(&prefix, &session_id)
                .to_string()
                .starts_with(&expected)
        );
        assert!(
            client_ops::TerminalOutputSubject::new(&prefix, &session_id)
                .to_string()
                .starts_with(&expected)
        );
        assert!(
            client_ops::TerminalReleaseSubject::new(&prefix, &session_id)
                .to_string()
                .starts_with(&expected)
        );
        assert!(
            client_ops::TerminalWaitForExitSubject::new(&prefix, &session_id)
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
            commands::LoadSubject::new(&prefix, &sid).to_subject().as_str(),
            "acp.session.s1.agent.load"
        );
        assert_eq!(
            commands::CancelSubject::new(&prefix, &sid).to_subject().as_str(),
            "acp.session.s1.agent.cancel"
        );
        assert_eq!(
            commands::CloseSubject::new(&prefix, &sid).to_subject().as_str(),
            "acp.session.s1.agent.close"
        );
        assert_eq!(
            commands::ForkSubject::new(&prefix, &sid).to_subject().as_str(),
            "acp.session.s1.agent.fork"
        );
        assert_eq!(
            commands::ResumeSubject::new(&prefix, &sid).to_subject().as_str(),
            "acp.session.s1.agent.resume"
        );
        assert_eq!(
            commands::SetModeSubject::new(&prefix, &sid).to_subject().as_str(),
            "acp.session.s1.agent.set_mode"
        );
        assert_eq!(
            commands::SetConfigOptionSubject::new(&prefix, &sid)
                .to_subject()
                .as_str(),
            "acp.session.s1.agent.set_config_option"
        );
        assert_eq!(
            commands::SetModelSubject::new(&prefix, &sid).to_subject().as_str(),
            "acp.session.s1.agent.set_model"
        );
        // ExtNotifySubject doesn't impl ToSubject — verify via Display
        assert_eq!(
            global::ExtNotifySubject::new(&prefix, &method("my_tool")).to_string(),
            "acp.agent.ext.my_tool"
        );

        // Subscription subjects
        assert_eq!(
            subscriptions::AllSessionSubject::new(&prefix).to_subject().as_str(),
            "acp.session.>"
        );
        assert_eq!(
            subscriptions::OneSessionSubject::new(&prefix, &sid)
                .to_subject()
                .as_str(),
            "acp.session.s1.>"
        );
        assert_eq!(
            subscriptions::OneAgentSubject::new(&prefix, &sid).to_subject().as_str(),
            "acp.session.s1.agent.>"
        );
        assert_eq!(
            subscriptions::OneClientSubject::new(&prefix, &sid)
                .to_subject()
                .as_str(),
            "acp.session.s1.client.>"
        );
        assert_eq!(
            subscriptions::PromptWildcardSubject::new(&prefix).to_subject().as_str(),
            "acp.session.*.agent.prompt"
        );

        // Response subject
        assert_eq!(
            responses::ResponseSubject::new(&prefix, &sid, &rid("req-abc"))
                .to_subject()
                .as_str(),
            "acp.session.s1.agent.response.req-abc"
        );
    }
