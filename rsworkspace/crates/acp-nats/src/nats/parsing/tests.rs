use super::*;

fn session(sid: &str, method: SessionAgentMethod) -> ParsedAgentSubject {
    ParsedAgentSubject::Session {
        session_id: AcpSessionId::new(sid).unwrap(),
        method,
    }
}

// Agent global methods

#[test]
fn parse_agent_initialize() {
    assert_eq!(
        parse_agent_subject("acp.agent.initialize").unwrap(),
        ParsedAgentSubject::Global(GlobalAgentMethod::Initialize)
    );
}

#[test]
fn parse_agent_authenticate() {
    assert_eq!(
        parse_agent_subject("acp.agent.authenticate").unwrap(),
        ParsedAgentSubject::Global(GlobalAgentMethod::Authenticate)
    );
}

#[test]
fn parse_agent_logout() {
    assert_eq!(
        parse_agent_subject("acp.agent.logout").unwrap(),
        ParsedAgentSubject::Global(GlobalAgentMethod::Logout)
    );
}

#[test]
fn parse_agent_session_new() {
    assert_eq!(
        parse_agent_subject("acp.agent.session.new").unwrap(),
        ParsedAgentSubject::Global(GlobalAgentMethod::SessionNew)
    );
}

#[test]
fn parse_agent_session_list() {
    assert_eq!(
        parse_agent_subject("acp.agent.session.list").unwrap(),
        ParsedAgentSubject::Global(GlobalAgentMethod::SessionList)
    );
}

#[test]
fn parse_agent_ext() {
    assert_eq!(
        parse_agent_subject("acp.agent.ext.my_tool").unwrap(),
        ParsedAgentSubject::Global(GlobalAgentMethod::Ext(ExtMethodName::new("my_tool").unwrap()))
    );
}

#[test]
fn parse_agent_ext_dotted() {
    assert_eq!(
        parse_agent_subject("acp.agent.ext.vendor.op").unwrap(),
        ParsedAgentSubject::Global(GlobalAgentMethod::Ext(ExtMethodName::new("vendor.op").unwrap()))
    );
}

// Session-scoped agent methods

#[test]
fn parse_session_agent_load() {
    assert_eq!(
        parse_agent_subject("acp.session.s1.agent.load").unwrap(),
        session("s1", SessionAgentMethod::Load)
    );
}

#[test]
fn parse_session_agent_prompt() {
    assert_eq!(
        parse_agent_subject("acp.session.s1.agent.prompt").unwrap(),
        session("s1", SessionAgentMethod::Prompt)
    );
}

#[test]
fn parse_session_agent_cancel() {
    assert_eq!(
        parse_agent_subject("acp.session.s1.agent.cancel").unwrap(),
        session("s1", SessionAgentMethod::Cancel)
    );
}

#[test]
fn parse_session_agent_set_mode() {
    assert_eq!(
        parse_agent_subject("acp.session.s1.agent.set_mode").unwrap(),
        session("s1", SessionAgentMethod::SetMode)
    );
}

#[test]
fn parse_session_agent_set_config_option() {
    assert_eq!(
        parse_agent_subject("acp.session.s1.agent.set_config_option").unwrap(),
        session("s1", SessionAgentMethod::SetConfigOption)
    );
}

#[test]
fn parse_session_agent_fork() {
    assert_eq!(
        parse_agent_subject("acp.session.s1.agent.fork").unwrap(),
        session("s1", SessionAgentMethod::Fork)
    );
}

#[test]
fn parse_session_agent_resume() {
    assert_eq!(
        parse_agent_subject("acp.session.s1.agent.resume").unwrap(),
        session("s1", SessionAgentMethod::Resume)
    );
}

#[test]
fn parse_session_agent_close() {
    assert_eq!(
        parse_agent_subject("acp.session.s1.agent.close").unwrap(),
        session("s1", SessionAgentMethod::Close)
    );
}

#[test]
fn parse_agent_custom_prefix() {
    assert_eq!(
        parse_agent_subject("myapp.agent.initialize").unwrap(),
        ParsedAgentSubject::Global(GlobalAgentMethod::Initialize)
    );
}

#[test]
fn parse_session_agent_custom_prefix() {
    assert_eq!(
        parse_agent_subject("myapp.session.s1.agent.prompt").unwrap(),
        session("s1", SessionAgentMethod::Prompt)
    );
}

#[test]
fn parse_agent_empty_returns_none() {
    assert!(parse_agent_subject("").is_none());
}

#[test]
fn parse_agent_unknown_method_returns_none() {
    assert!(parse_agent_subject("acp.agent.unknown").is_none());
}

#[test]
fn parse_agent_invalid_session_id_returns_none() {
    assert!(parse_agent_subject("acp.session.s*1.agent.load").is_none());
}

#[test]
fn parse_agent_client_subject_returns_none() {
    assert!(parse_agent_subject("acp.session.s1.client.session.update").is_none());
}

#[test]
fn parse_agent_no_overlap_with_session_id_agent() {
    assert_eq!(
        parse_agent_subject("acp.session.agent.agent.load").unwrap(),
        session("agent", SessionAgentMethod::Load)
    );
}

// Client methods

#[test]
fn parse_client_fs_read() {
    let parsed = parse_client_subject("acp.session.s1.client.fs.read_text_file").unwrap();
    assert_eq!(parsed.session_id.as_str(), "s1");
    assert_eq!(parsed.method, ClientMethod::FsReadTextFile);
}

#[test]
fn parse_client_fs_write() {
    let parsed = parse_client_subject("acp.session.s1.client.fs.write_text_file").unwrap();
    assert_eq!(parsed.session_id.as_str(), "s1");
    assert_eq!(parsed.method, ClientMethod::FsWriteTextFile);
}

#[test]
fn parse_client_request_permission() {
    let parsed = parse_client_subject("acp.session.s1.client.session.request_permission").unwrap();
    assert_eq!(parsed.session_id.as_str(), "s1");
    assert_eq!(parsed.method, ClientMethod::SessionRequestPermission);
}

#[test]
fn parse_client_session_update() {
    let parsed = parse_client_subject("acp.session.s1.client.session.update").unwrap();
    assert_eq!(parsed.session_id.as_str(), "s1");
    assert_eq!(parsed.method, ClientMethod::SessionUpdate);
}

#[test]
fn parse_client_terminal_create() {
    let parsed = parse_client_subject("acp.session.s1.client.terminal.create").unwrap();
    assert_eq!(parsed.session_id.as_str(), "s1");
    assert_eq!(parsed.method, ClientMethod::TerminalCreate);
}

#[test]
fn parse_client_terminal_kill() {
    let parsed = parse_client_subject("acp.session.s1.client.terminal.kill").unwrap();
    assert_eq!(parsed.session_id.as_str(), "s1");
    assert_eq!(parsed.method, ClientMethod::TerminalKill);
}

#[test]
fn parse_client_terminal_output() {
    let parsed = parse_client_subject("acp.session.s1.client.terminal.output").unwrap();
    assert_eq!(parsed.session_id.as_str(), "s1");
    assert_eq!(parsed.method, ClientMethod::TerminalOutput);
}

#[test]
fn parse_client_terminal_release() {
    let parsed = parse_client_subject("acp.session.s1.client.terminal.release").unwrap();
    assert_eq!(parsed.session_id.as_str(), "s1");
    assert_eq!(parsed.method, ClientMethod::TerminalRelease);
}

#[test]
fn parse_client_terminal_wait() {
    let parsed = parse_client_subject("acp.session.s1.client.terminal.wait_for_exit").unwrap();
    assert_eq!(parsed.session_id.as_str(), "s1");
    assert_eq!(parsed.method, ClientMethod::TerminalWaitForExit);
}

#[test]
fn parse_client_ext_prompt_response() {
    let parsed = parse_client_subject("acp.session.s1.client.ext.session.prompt_response").unwrap();
    assert_eq!(parsed.session_id.as_str(), "s1");
    assert_eq!(parsed.method, ClientMethod::ExtSessionPromptResponse);
}

#[test]
fn parse_client_custom_prefix() {
    let parsed = parse_client_subject("myapp.session.s1.client.fs.read_text_file").unwrap();
    assert_eq!(parsed.session_id.as_str(), "s1");
    assert_eq!(parsed.method, ClientMethod::FsReadTextFile);
}

#[test]
fn parse_client_empty_returns_none() {
    assert!(parse_client_subject("").is_none());
}

#[test]
fn parse_client_no_session_returns_none() {
    assert!(parse_client_subject("acp.client.fs.read_text_file").is_none());
}

#[test]
fn parse_client_unknown_method_returns_none() {
    assert!(parse_client_subject("acp.session.s1.client.unknown").is_none());
}

#[test]
fn parse_client_session_but_no_valid_method_returns_none() {
    assert!(parse_client_subject("acp.session.s1.client.nope").is_none());
}

#[test]
fn parse_client_prefix_containing_session_word() {
    let parsed = parse_client_subject("my.session.app.session.s1.client.fs.read_text_file").unwrap();
    assert_eq!(parsed.session_id.as_str(), "s1");
    assert_eq!(parsed.method, ClientMethod::FsReadTextFile);
}

#[test]
fn parse_session_agent_unknown_method_returns_none() {
    assert!(parse_agent_subject("acp.session.s1.agent.unknown").is_none());
}

#[test]
fn parse_agent_prefix_containing_agent_word() {
    assert_eq!(
        parse_agent_subject("org.agent.app.agent.initialize").unwrap(),
        ParsedAgentSubject::Global(GlobalAgentMethod::Initialize)
    );
}

#[test]
fn parse_session_agent_prefix_containing_session_word() {
    assert_eq!(
        parse_agent_subject("my.session.app.session.s1.agent.prompt").unwrap(),
        session("s1", SessionAgentMethod::Prompt)
    );
}

#[test]
fn parse_agent_prefix_containing_session_falls_through_to_global() {
    assert_eq!(
        parse_agent_subject("my.session.handler.agent.initialize").unwrap(),
        ParsedAgentSubject::Global(GlobalAgentMethod::Initialize)
    );
}

#[test]
fn parse_client_ext_method() {
    let parsed = parse_client_subject("acp.session.s1.client.ext.my_tool").unwrap();
    assert_eq!(parsed.session_id.as_str(), "s1");
    assert_eq!(parsed.method, ClientMethod::Ext("my_tool".to_string()));
}
