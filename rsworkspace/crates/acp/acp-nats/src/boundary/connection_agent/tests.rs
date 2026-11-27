use super::*;
use crate::boundary::{BoundaryExit, connect_agent_boundary, connect_client_boundary_with};
use crate::client_handler::ClientHandler;
use agent_client_protocol::ByteStreams;
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::v1::schema::{
    RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse, SessionNotification, StopReason,
};
use std::sync::{Arc, Mutex};

/// Records which handler methods were reached, so a test can assert that a
/// call made through [`ConnectionAgent`] arrived as the same method.
#[derive(Default)]
struct RecordingAgent {
    calls: Mutex<Vec<&'static str>>,
    ext_methods: Mutex<Vec<String>>,
}

impl RecordingAgent {
    fn record(&self, method: &'static str) -> Result<()> {
        self.calls.lock().map_err(|_| Error::internal_error())?.push(method);
        Ok(())
    }
}

#[async_trait::async_trait]
impl AgentHandler for RecordingAgent {
    async fn initialize(&self, args: InitializeRequest) -> Result<InitializeResponse> {
        self.record("initialize")?;
        Ok(InitializeResponse::new(args.protocol_version))
    }

    async fn authenticate(&self, _args: AuthenticateRequest) -> Result<AuthenticateResponse> {
        self.record("authenticate")?;
        Ok(AuthenticateResponse::new())
    }

    async fn new_session(&self, _args: NewSessionRequest) -> Result<NewSessionResponse> {
        self.record("new_session")?;
        Ok(NewSessionResponse::new("sess-1"))
    }

    async fn prompt(&self, _args: PromptRequest) -> Result<PromptResponse> {
        self.record("prompt")?;
        Ok(PromptResponse::new(StopReason::EndTurn))
    }

    async fn cancel(&self, _args: CancelNotification) -> Result<()> {
        self.record("cancel")
    }

    async fn close_session(&self, _args: CloseSessionRequest) -> Result<CloseSessionResponse> {
        self.record("close_session")?;
        Ok(CloseSessionResponse::new())
    }

    async fn ext_method(&self, args: ExtRequest) -> Result<ExtResponse> {
        self.record("ext_method")?;
        self.record_ext(&args.method)?;
        Ok(ExtResponse::new(args.params))
    }

    async fn ext_notification(&self, args: ExtNotification) -> Result<()> {
        self.record_ext(&args.method)
    }
}

impl RecordingAgent {
    fn record_ext(&self, method: &str) -> Result<()> {
        self.ext_methods
            .lock()
            .map_err(|_| Error::internal_error())?
            .push(method.to_string());
        Ok(())
    }
}

struct NoopClient;

#[async_trait::async_trait]
impl ClientHandler for NoopClient {
    async fn request_permission(&self, _args: RequestPermissionRequest) -> Result<RequestPermissionResponse> {
        Ok(RequestPermissionResponse::new(RequestPermissionOutcome::Cancelled))
    }

    async fn session_notification(&self, _args: SessionNotification) -> Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn each_call_reaches_the_agent_as_the_same_method() {
    let (client_io, agent_io) = tokio::io::duplex(64 * 1024);
    let (client_read, client_write) = tokio::io::split(client_io);
    let (agent_read, agent_write) = tokio::io::split(agent_io);

    let agent = Arc::new(RecordingAgent::default());
    tokio::spawn(connect_agent_boundary(
        agent.clone(),
        async_compat::Compat::new(agent_write),
        async_compat::Compat::new(agent_read),
        async move |_cx| std::future::pending::<Result<()>>().await,
    ));

    let transport = ByteStreams::new(
        async_compat::Compat::new(client_write),
        async_compat::Compat::new(client_read),
    );
    let exit = connect_client_boundary_with(Arc::new(NoopClient), transport, async move |cx| {
        let peer = ConnectionAgent::new(cx);
        peer.initialize(InitializeRequest::new(ProtocolVersion::V1)).await?;
        peer.authenticate(AuthenticateRequest::new("none")).await?;
        let session = peer.new_session(NewSessionRequest::new("/tmp")).await?;
        peer.prompt(PromptRequest::new(session.session_id.clone(), vec![]))
            .await?;
        peer.cancel(CancelNotification::new(session.session_id.clone())).await?;
        peer.close_session(CloseSessionRequest::new(session.session_id.clone()))
            .await?;
        let params =
            serde_json::value::to_raw_value(&serde_json::json!({"k": 1})).map_err(Error::into_internal_error)?;
        let ext = peer.ext_method(ExtRequest::new("vendor.ping", params.into())).await?;
        // The handler a peer did not implement answers with its own error,
        // which the passthrough returns untouched.
        let unimplemented = peer.list_sessions(ListSessionsRequest::new()).await;
        Ok((ext, unimplemented.is_err()))
    })
    .await
    .expect("boundary runs");

    let BoundaryExit::Main((ext, unimplemented_failed)) = exit else {
        panic!("connection closed early");
    };
    assert_eq!(ext.0.get(), r#"{"k":1}"#);
    assert!(unimplemented_failed);

    // `session/cancel` is a notification, so it may land after the next request
    // has already been answered; wait for it rather than racing it.
    let expected = [
        "initialize",
        "authenticate",
        "new_session",
        "prompt",
        "cancel",
        "close_session",
        "ext_method",
    ];
    for _ in 0..100 {
        if agent.calls.lock().expect("lock").len() == expected.len() {
            break;
        }
        tokio::task::yield_now().await;
    }
    let mut calls = agent.calls.lock().expect("lock").clone();
    let mut want = expected.to_vec();
    calls.sort_unstable();
    want.sort_unstable();
    assert_eq!(calls, want);
}

#[tokio::test]
async fn an_extension_name_reaches_the_agent_with_exactly_one_underscore() {
    let (client_io, agent_io) = tokio::io::duplex(64 * 1024);
    let (client_read, client_write) = tokio::io::split(client_io);
    let (agent_read, agent_write) = tokio::io::split(agent_io);

    let agent = Arc::new(RecordingAgent::default());
    tokio::spawn(connect_agent_boundary(
        agent.clone(),
        async_compat::Compat::new(agent_write),
        async_compat::Compat::new(agent_read),
        async move |_cx| std::future::pending::<Result<()>>().await,
    ));

    let transport = ByteStreams::new(
        async_compat::Compat::new(client_write),
        async_compat::Compat::new(client_read),
    );
    let exit = connect_client_boundary_with(Arc::new(NoopClient), transport, async move |cx| {
        let peer = ConnectionAgent::new(cx);
        peer.initialize(InitializeRequest::new(ProtocolVersion::V1)).await?;
        let params = || serde_json::value::to_raw_value(&serde_json::json!({})).map_err(Error::into_internal_error);
        // A name relayed from another ACP peer already carries its `_`.
        peer.ext_method(ExtRequest::new("_vendor.ping", params()?.into()))
            .await?;
        peer.ext_method(ExtRequest::new("vendor.ping", params()?.into()))
            .await?;
        peer.ext_notification(ExtNotification::new("_vendor.note", params()?.into()))
            .await?;
        peer.ext_notification(ExtNotification::new("vendor.note", params()?.into()))
            .await?;
        // A request after the notifications, so they have been delivered.
        peer.ext_method(ExtRequest::new("_vendor.ping", params()?.into()))
            .await?;
        Ok(())
    })
    .await
    .expect("boundary runs");

    assert!(matches!(exit, BoundaryExit::Main(())), "connection closed early");
    let mut received = agent.ext_methods.lock().expect("lock").clone();
    received.sort_unstable();
    assert_eq!(
        received,
        [
            "_vendor.note",
            "_vendor.note",
            "_vendor.ping",
            "_vendor.ping",
            "_vendor.ping"
        ]
    );
}
