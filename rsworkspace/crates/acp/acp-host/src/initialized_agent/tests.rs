use super::*;
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::v1::schema::{AgentCapabilities, ClientCapabilities, Implementation, StopReason};
use std::sync::atomic::{AtomicU32, Ordering};

#[derive(Default)]
struct RecordingAgent {
    initializes: std::sync::Mutex<Vec<InitializeRequest>>,
    prompts: AtomicU32,
    refuses_initialize: bool,
    /// When set, every `initialize` waits for a permit before it answers.
    paused: Option<Arc<tokio::sync::Semaphore>>,
}

impl RecordingAgent {
    fn initializes(&self) -> Vec<InitializeRequest> {
        self.initializes.lock().unwrap().clone()
    }
}

fn reply_to(args: &InitializeRequest) -> InitializeResponse {
    InitializeResponse::new(args.protocol_version)
        .agent_capabilities(AgentCapabilities::new().load_session(true))
        .agent_info(Implementation::new("fake-agent", "1"))
}

#[async_trait::async_trait]
impl AgentHandler for RecordingAgent {
    async fn initialize(&self, args: InitializeRequest) -> Result<InitializeResponse> {
        if self.refuses_initialize {
            return Err(agent_client_protocol::Error::invalid_params());
        }
        if let Some(paused) = &self.paused {
            paused.acquire().await.expect("open").forget();
        }
        let reply = reply_to(&args);
        self.initializes.lock().unwrap().push(args);
        Ok(reply)
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

struct Harness {
    agent: Arc<InitializedAgent<RecordingAgent>>,
    handshake: Arc<ClientHandshake>,
}

fn harness(initialized: Option<Handshake>) -> Harness {
    let handshake = Arc::new(ClientHandshake::default());
    let agent = Arc::new(InitializedAgent::new(
        RecordingAgent::default(),
        initialized,
        Arc::default(),
        handshake.clone(),
    ));
    Harness { agent, handshake }
}

fn meta(value: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
    match value {
        serde_json::Value::Object(map) => map,
        _ => panic!("meta must be an object"),
    }
}

/// What a client such as fx's host-tools bridge sends: capabilities with
/// `_meta`, its own `clientInfo`, and `_meta` on the request.
fn discord(version: &str) -> InitializeRequest {
    InitializeRequest::new(ProtocolVersion::V1)
        .client_capabilities(ClientCapabilities::new().meta(meta(
            serde_json::json!({"libtrogonai.tools": [{"name": "send_message"}]}),
        )))
        .client_info(Implementation::new("discord", version))
        .meta(meta(serde_json::json!({"traceparent": "00-abc-def-01"})))
}

#[tokio::test]
async fn the_first_initialize_reaches_the_child_as_sent_and_returns_its_answer() {
    let h = harness(None);

    let answer = h.agent.initialize(discord("1")).await.expect("initialize");

    assert_eq!(h.agent.inner.initializes(), vec![discord("1")]);
    assert_eq!(answer, reply_to(&discord("1")));
    assert_eq!(h.handshake.next(), Some(discord("1").into()), "kept for the next child");
}

#[tokio::test]
async fn the_same_initialize_again_is_answered_without_the_child() {
    let h = harness(None);

    let first = h.agent.initialize(discord("1")).await.expect("first");
    let again = h.agent.initialize(discord("1")).await.expect("again");

    assert_eq!(first, again);
    assert_eq!(h.agent.inner.initializes().len(), 1);
}

#[tokio::test]
async fn a_child_the_run_already_initialized_is_not_initialized_again() {
    let request = discord("1");
    let h = harness(Some(Handshake {
        response: reply_to(&request),
        request: request.clone().into(),
    }));

    h.agent.initialize(request).await.expect("initialize");
    h.agent
        .prompt(PromptRequest::new("sess-1", vec![]))
        .await
        .expect("prompt");

    assert!(h.agent.inner.initializes().is_empty());
}

#[tokio::test]
async fn a_different_initialize_relaunches_the_child_and_returns_the_new_childs_answer() {
    let h = harness(None);
    h.agent.initialize(discord("1")).await.expect("first");

    let agent = h.agent.clone();
    let reinitialize = tokio::spawn(async move { agent.initialize(discord("2")).await });
    h.handshake.relaunch_wanted().await;

    assert_eq!(
        h.handshake.next(),
        Some(discord("2").into()),
        "the next child gets the new one"
    );
    let new_childs_answer =
        InitializeResponse::new(ProtocolVersion::V1).agent_info(Implementation::new("new-child", "1"));
    h.handshake.settle(&discord("2").into(), &Ok(new_childs_answer.clone()));

    assert_eq!(reinitialize.await.unwrap().expect("answered"), new_childs_answer);
    assert_eq!(
        h.agent.inner.initializes(),
        vec![discord("1")],
        "the running child never gets a second initialize"
    );
}

#[tokio::test]
async fn a_request_before_any_initialize_initializes_the_child_with_defaults_first() {
    let h = harness(None);

    h.agent
        .prompt(PromptRequest::new("sess-1", vec![]))
        .await
        .expect("prompt");
    h.agent
        .prompt(PromptRequest::new("sess-1", vec![]))
        .await
        .expect("prompt");

    assert_eq!(
        h.agent.inner.initializes(),
        vec![InitializeRequest::new(FALLBACK_PROTOCOL_VERSION)]
    );
    assert_eq!(h.agent.inner.prompts.load(Ordering::SeqCst), 2);
    assert_eq!(h.handshake.next(), None, "defaults are not the client's to keep");
}

#[tokio::test]
async fn the_clients_initialize_after_defaults_relaunches_the_child_with_it() {
    let h = harness(None);
    h.agent
        .prompt(PromptRequest::new("sess-1", vec![]))
        .await
        .expect("prompt");

    let agent = h.agent.clone();
    let reinitialize = tokio::spawn(async move { agent.initialize(discord("1")).await });
    h.handshake.relaunch_wanted().await;
    h.handshake.settle(&discord("1").into(), &Ok(reply_to(&discord("1"))));

    assert_eq!(reinitialize.await.unwrap().expect("answered"), reply_to(&discord("1")));
}

#[tokio::test]
async fn a_second_client_is_refused_before_the_child_sees_it() {
    let h = harness(None);
    h.agent.initialize(discord("1")).await.expect("first");

    let refusal = h.agent.initialize(zed()).await.expect_err("refused");

    assert_eq!(refusal.code, agent_client_protocol::ErrorCode::INVALID_REQUEST);
    assert_eq!(h.agent.inner.initializes().len(), 1);
}

fn zed() -> InitializeRequest {
    InitializeRequest::new(ProtocolVersion::V1).client_info(Implementation::new("zed", "1"))
}

#[tokio::test]
async fn a_client_that_loses_the_race_for_the_claim_is_refused_instead_of_relaunching_the_winner() {
    let paused = Arc::new(tokio::sync::Semaphore::new(0));
    let handshake = Arc::new(ClientHandshake::default());
    let child = RecordingAgent {
        paused: Some(paused.clone()),
        ..RecordingAgent::default()
    };
    let agent = Arc::new(InitializedAgent::new(child, None, Arc::default(), handshake.clone()));

    let winner = tokio::spawn({
        let agent = agent.clone();
        async move { agent.initialize(discord("1")).await }
    });
    while agent.child.try_lock().is_ok() {
        tokio::task::yield_now().await;
    }
    // The winner is inside the child's `initialize`, holding the child, while
    // the host is still unclaimed; the loser arrives now and waits its turn.
    let loser = tokio::spawn({
        let agent = agent.clone();
        async move { agent.initialize(zed()).await }
    });
    for _ in 0..10 {
        tokio::task::yield_now().await;
    }
    paused.add_permits(1);

    assert_eq!(winner.await.unwrap().expect("winner"), reply_to(&discord("1")));
    let refusal = loser.await.unwrap().expect_err("loser refused");
    assert_eq!(refusal.code, agent_client_protocol::ErrorCode::INVALID_REQUEST);
    assert_eq!(agent.inner.initializes(), vec![discord("1")]);
    assert_eq!(
        handshake.next(),
        Some(discord("1").into()),
        "the winner's child is not relaunched"
    );
}

#[tokio::test]
async fn a_client_that_asks_for_a_relaunch_claims_the_host_before_another_can() {
    let h = harness(None);
    h.agent
        .prompt(PromptRequest::new("sess-1", vec![]))
        .await
        .expect("prompt");
    let first = tokio::spawn({
        let agent = h.agent.clone();
        async move { agent.initialize(discord("1")).await }
    });
    h.handshake.relaunch_wanted().await;

    let refusal = h.agent.initialize(zed()).await.expect_err("second client refused");

    assert_eq!(refusal.code, agent_client_protocol::ErrorCode::INVALID_REQUEST);
    assert_eq!(
        h.handshake.next(),
        Some(discord("1").into()),
        "the relaunch keeps the first client's initialize"
    );
    h.handshake.settle(&discord("1").into(), &Ok(reply_to(&discord("1"))));
    assert_eq!(first.await.unwrap().expect("answered"), reply_to(&discord("1")));
}

#[tokio::test]
async fn an_initialize_the_child_refuses_claims_nothing() {
    let claim = Arc::new(ClientClaim::default());
    let child = RecordingAgent {
        refuses_initialize: true,
        ..RecordingAgent::default()
    };
    let agent = InitializedAgent::new(child, None, claim.clone(), Arc::default());
    let squatter = InitializeRequest::new(ProtocolVersion::V1).client_info(Implementation::new("squatter", "1"));

    agent.initialize(squatter).await.expect_err("refused by the child");

    claim.claim(&discord("1")).expect("the real client is not locked out");
}

#[tokio::test]
async fn other_methods_reach_the_agent() {
    let h = harness(None);
    h.agent.initialize(discord("1")).await.expect("initialize");

    h.agent
        .prompt(PromptRequest::new("sess-1", vec![]))
        .await
        .expect("prompt");
    let unimplemented = h.agent.list_sessions(ListSessionsRequest::new()).await;

    assert_eq!(h.agent.inner.prompts.load(Ordering::SeqCst), 1);
    assert!(
        unimplemented.is_err(),
        "an agent default stays an error, not a host answer"
    );
}
