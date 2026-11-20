use super::*;
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::v1::schema::{AgentCapabilities, StopReason};
use std::sync::atomic::{AtomicU32, Ordering};

#[derive(Default)]
struct CountingAgent {
    initializes: AtomicU32,
    prompts: AtomicU32,
}

#[async_trait::async_trait]
impl AgentHandler for CountingAgent {
    async fn initialize(&self, args: InitializeRequest) -> Result<InitializeResponse> {
        self.initializes.fetch_add(1, Ordering::SeqCst);
        Ok(InitializeResponse::new(args.protocol_version))
    }

    async fn authenticate(&self, _args: AuthenticateRequest) -> Result<AuthenticateResponse> {
        Ok(AuthenticateResponse::new())
    }

    async fn new_session(&self, _args: NewSessionRequest) -> Result<NewSessionResponse> {
        Ok(NewSessionResponse::new("sess-1"))
    }

    async fn prompt(&self, _args: PromptRequest) -> Result<PromptResponse> {
        self.prompts.fetch_add(1, Ordering::SeqCst);
        Ok(PromptResponse::new(StopReason::EndTurn))
    }

    async fn cancel(&self, _args: CancelNotification) -> Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn every_client_initialize_is_answered_from_the_hosts_own_handshake() {
    let reply =
        InitializeResponse::new(ProtocolVersion::V1).agent_capabilities(AgentCapabilities::new().load_session(true));
    let agent = InitializedAgent::new(CountingAgent::default(), reply.clone());

    for _ in 0..3 {
        let answer = agent
            .initialize(InitializeRequest::new(ProtocolVersion::V1))
            .await
            .expect("initialize");
        assert_eq!(answer, reply);
    }
    assert_eq!(agent.inner.initializes.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn other_methods_reach_the_agent() {
    let agent = InitializedAgent::new(CountingAgent::default(), InitializeResponse::new(ProtocolVersion::V1));

    agent
        .prompt(PromptRequest::new("sess-1", vec![]))
        .await
        .expect("prompt");
    let unimplemented = agent.list_sessions(ListSessionsRequest::new()).await;

    assert_eq!(agent.inner.prompts.load(Ordering::SeqCst), 1);
    assert!(
        unimplemented.is_err(),
        "an agent default stays an error, not a host answer"
    );
}
