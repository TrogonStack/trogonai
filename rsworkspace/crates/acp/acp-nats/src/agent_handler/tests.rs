use super::*;
use agent_client_protocol::ErrorCode;
use agent_client_protocol::schema::v1::{LlmProtocol, SessionConfigOptionValue, StopReason};
use std::sync::Arc;

struct MinimalAgent;

#[async_trait::async_trait]
impl AgentHandler for MinimalAgent {
    async fn initialize(&self, args: InitializeRequest) -> Result<InitializeResponse> {
        Ok(InitializeResponse::new(args.protocol_version))
    }

    async fn authenticate(&self, _args: AuthenticateRequest) -> Result<AuthenticateResponse> {
        Ok(AuthenticateResponse::new())
    }

    async fn new_session(&self, _args: NewSessionRequest) -> Result<NewSessionResponse> {
        Ok(NewSessionResponse::new("s1"))
    }

    async fn prompt(&self, _args: PromptRequest) -> Result<PromptResponse> {
        Ok(PromptResponse::new(StopReason::EndTurn))
    }

    async fn cancel(&self, _args: CancelNotification) -> Result<()> {
        Ok(())
    }
}

fn assert_method_not_found<T>(result: Result<T>) {
    let error = result.err().unwrap();
    assert_eq!(error.code, ErrorCode::MethodNotFound);
}

fn empty_raw_value() -> Arc<serde_json::value::RawValue> {
    serde_json::value::RawValue::from_string("{}".to_string())
        .unwrap()
        .into()
}

#[tokio::test]
async fn optional_methods_default_to_method_not_found() {
    let agent = MinimalAgent;

    assert_method_not_found(agent.logout(LogoutRequest::new()).await);
    assert_method_not_found(agent.load_session(LoadSessionRequest::new("s1", ".")).await);
    assert_method_not_found(agent.set_session_mode(SetSessionModeRequest::new("s1", "mode")).await);
    assert_method_not_found(
        agent
            .set_session_config_option(SetSessionConfigOptionRequest::new(
                "s1",
                "verbose",
                SessionConfigOptionValue::boolean(true),
            ))
            .await,
    );
    assert_method_not_found(agent.fork_session(ForkSessionRequest::new("s1", ".")).await);
    assert_method_not_found(agent.resume_session(ResumeSessionRequest::new("s1", ".")).await);
    assert_method_not_found(agent.close_session(CloseSessionRequest::new("s1")).await);
    assert_method_not_found(agent.delete_session(DeleteSessionRequest::new("s1")).await);
    assert_method_not_found(agent.list_sessions(ListSessionsRequest::new()).await);
    assert_method_not_found(agent.ext_method(ExtRequest::new("ext", empty_raw_value())).await);
    assert_method_not_found(
        agent
            .ext_notification(ExtNotification::new("ext", empty_raw_value()))
            .await,
    );
    assert_method_not_found(agent.list_providers(ListProvidersRequest::new()).await);
    assert_method_not_found(
        agent
            .set_provider(SetProviderRequest::new(
                "anthropic",
                LlmProtocol::Anthropic,
                "https://api.example.com",
            ))
            .await,
    );
    assert_method_not_found(agent.disable_provider(DisableProviderRequest::new("anthropic")).await);
}
