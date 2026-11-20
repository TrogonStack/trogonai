//! IDE-facing ACP v1 JSON-RPC connection.
//!
//! Replaces the `AgentSideConnection` that `agent-client-protocol` 0.10 provided.
//! It serves an [`acp::Agent`] (normally the NATS [`Bridge`](crate::Bridge))
//! over a newline-delimited JSON-RPC byte stream, and the returned
//! [`AgentSideConnection`] implements [`acp::Client`] so the bridge can call
//! back into the IDE.
//!
//! The SDK requires `Send` handlers, while the bridge is `!Send` and runs on a
//! `tokio::task::LocalSet`. Incoming messages are therefore forwarded from the
//! SDK's handler over a channel to the local task that owns the agent, which
//! spawns one local task per message (as the 0.10 SDK did).

use std::future::Future;
use std::rc::Rc;

use agent_client_protocol as sdk;
use serde::Serialize;
use tokio::sync::{mpsc, watch};
use tracing::warn;

use crate::acp::{self, Agent};

type IdeConnection = sdk::ConnectionTo<sdk::Client>;

enum Inbound {
    Request(Box<(acp::ClientRequest, sdk::Responder)>),
    Notification(acp::ClientNotification),
}

/// Handle to the IDE side of an ACP connection. Implements [`acp::Client`].
#[derive(Clone)]
pub struct AgentSideConnection {
    connection: watch::Receiver<Option<IdeConnection>>,
}

impl AgentSideConnection {
    /// Serves `agent` over `outgoing` / `incoming`.
    ///
    /// Returns the client handle and the I/O future that drives the connection.
    /// The future must be polled on a `LocalSet`; it completes when the
    /// incoming stream closes.
    pub fn new<A, W, R>(agent: Rc<A>, outgoing: W, incoming: R) -> (Self, impl Future<Output = acp::Result<()>>)
    where
        A: Agent + 'static,
        W: futures::AsyncWrite + Send + 'static,
        R: futures::AsyncRead + Send + 'static,
    {
        let (connection_tx, connection_rx) = watch::channel(None);
        let (inbound_tx, mut inbound_rx) = mpsc::unbounded_channel::<Inbound>();

        let io = async move {
            // The raw builder applies no protocol-version guard: the bridge is a
            // pass-through, so version negotiation stays between the IDE and the
            // backend agent behind NATS (as with the 0.10 SDK).
            sdk::Builder::new(sdk::Agent)
                .name("acp-nats")
                .on_receive_dispatch(
                    async move |dispatch: sdk::Dispatch<acp::ClientRequest, acp::ClientNotification>, _cx| {
                        let inbound = match dispatch {
                            sdk::Dispatch::Request(request, responder) => {
                                Inbound::Request(Box::new((request, responder)))
                            }
                            sdk::Dispatch::Notification(notification) => Inbound::Notification(notification),
                            // Responses to requests we sent to the IDE pass through here too;
                            // route them back to the waiting `send_request` call.
                            sdk::Dispatch::Response(result, router) => return router.route_with_result(result),
                        };
                        if inbound_tx.send(inbound).is_err() {
                            warn!("ACP agent task is gone; dropping incoming message");
                        }
                        Ok(())
                    },
                    sdk::on_receive_dispatch!(),
                )
                .connect_with(sdk::ByteStreams::new(outgoing, incoming), async move |cx| {
                    let _ = connection_tx.send(Some(cx.clone()));
                    loop {
                        tokio::select! {
                            message = inbound_rx.recv() => {
                                let Some(message) = message else { break };
                                let agent = agent.clone();
                                tokio::task::spawn_local(async move { dispatch(&*agent, message).await });
                            }
                            () = cx.incoming_closed() => break,
                        }
                    }
                    Ok(())
                })
                .await
                .map_err(acp::Error::from)
        };

        (
            Self {
                connection: connection_rx,
            },
            io,
        )
    }

    async fn connection(&self) -> acp::Result<IdeConnection> {
        let mut rx = self.connection.clone();
        let connection = rx
            .wait_for(Option::is_some)
            .await
            .map_err(|_| acp::Error::internal_error().data("ACP connection closed"))?;
        Ok(connection.clone().expect("checked by wait_for"))
    }

    async fn request<Req: sdk::JsonRpcRequest>(&self, request: Req) -> acp::Result<Req::Response> {
        self.connection()
            .await?
            .send_request(request)
            .block_task()
            .await
            .map_err(acp::Error::from)
    }

    async fn notify<N: sdk::JsonRpcNotification>(&self, notification: N) -> acp::Result<()> {
        self.connection()
            .await?
            .send_notification(notification)
            .map_err(acp::Error::from)
    }
}

/// ACP extension methods travel with a leading `_` on the wire. The 0.10 SDK
/// stripped it before calling `ext_method` / `ext_notification` and added it back
/// when sending; keep that contract so the bridge's NATS subjects are unchanged.
fn strip_ext_prefix(method: &std::sync::Arc<str>) -> std::sync::Arc<str> {
    method.strip_prefix('_').unwrap_or(method).into()
}

fn ext_wire_method(method: &str) -> String {
    format!("_{method}")
}

fn to_value<T: Serialize>(result: acp::Result<T>) -> acp::Result<serde_json::Value> {
    result.and_then(|value| serde_json::to_value(value).map_err(acp::Error::into_internal_error))
}

async fn handle_request<A: Agent + ?Sized>(agent: &A, request: acp::ClientRequest) -> acp::Result<serde_json::Value> {
    use acp::ClientRequest as R;
    match request {
        R::InitializeRequest(r) => to_value(agent.initialize(r).await),
        R::AuthenticateRequest(r) => to_value(agent.authenticate(r).await),
        R::LogoutRequest(r) => to_value(agent.logout(r).await),
        R::NewSessionRequest(r) => to_value(agent.new_session(r).await),
        R::LoadSessionRequest(r) => to_value(agent.load_session(r).await),
        R::ListSessionsRequest(r) => to_value(agent.list_sessions(r).await),
        R::ForkSessionRequest(r) => to_value(agent.fork_session(r).await),
        R::ResumeSessionRequest(r) => to_value(agent.resume_session(r).await),
        R::CloseSessionRequest(r) => to_value(agent.close_session(r).await),
        R::SetSessionModeRequest(r) => to_value(agent.set_session_mode(r).await),
        R::SetSessionConfigOptionRequest(r) => to_value(agent.set_session_config_option(r).await),
        R::PromptRequest(r) => to_value(agent.prompt(r).await),
        R::ExtMethodRequest(mut r) => {
            r.method = strip_ext_prefix(&r.method);
            to_value(agent.ext_method(r).await)
        }
        // e.g. session/delete: not supported by the bridge.
        _ => Err(acp::Error::method_not_found()),
    }
}

async fn dispatch<A: Agent + ?Sized>(agent: &A, message: Inbound) {
    match message {
        Inbound::Request(request) => {
            let (request, responder) = *request;
            let result = handle_request(agent, request).await.map_err(sdk::Error::from);
            if let Err(e) = responder.respond_with_result(result) {
                warn!(error = %e, "Failed to send ACP response");
            }
        }
        Inbound::Notification(notification) => {
            let result = match notification {
                acp::ClientNotification::CancelNotification(n) => agent.cancel(n).await,
                acp::ClientNotification::ExtNotification(mut n) => {
                    n.method = strip_ext_prefix(&n.method);
                    agent.ext_notification(n).await
                }
                _ => Ok(()),
            };
            if let Err(e) = result {
                warn!(error = %e, "ACP notification handler failed");
            }
        }
    }
}

#[async_trait::async_trait(?Send)]
impl acp::Client for AgentSideConnection {
    async fn request_permission(
        &self,
        args: acp::RequestPermissionRequest,
    ) -> acp::Result<acp::RequestPermissionResponse> {
        self.request(args).await
    }

    async fn session_notification(&self, args: acp::SessionNotification) -> acp::Result<()> {
        self.notify(args).await
    }

    async fn write_text_file(&self, args: acp::WriteTextFileRequest) -> acp::Result<acp::WriteTextFileResponse> {
        self.request(args).await
    }

    async fn read_text_file(&self, args: acp::ReadTextFileRequest) -> acp::Result<acp::ReadTextFileResponse> {
        self.request(args).await
    }

    async fn create_terminal(&self, args: acp::CreateTerminalRequest) -> acp::Result<acp::CreateTerminalResponse> {
        self.request(args).await
    }

    async fn terminal_output(&self, args: acp::TerminalOutputRequest) -> acp::Result<acp::TerminalOutputResponse> {
        self.request(args).await
    }

    async fn release_terminal(&self, args: acp::ReleaseTerminalRequest) -> acp::Result<acp::ReleaseTerminalResponse> {
        self.request(args).await
    }

    async fn wait_for_terminal_exit(
        &self,
        args: acp::WaitForTerminalExitRequest,
    ) -> acp::Result<acp::WaitForTerminalExitResponse> {
        self.request(args).await
    }

    async fn kill_terminal(&self, args: acp::KillTerminalRequest) -> acp::Result<acp::KillTerminalResponse> {
        self.request(args).await
    }

    async fn ext_method(&self, args: acp::ExtRequest) -> acp::Result<acp::ExtResponse> {
        let message =
            sdk::UntypedMessage::new(&ext_wire_method(&args.method), &args.params).map_err(acp::Error::from)?;
        let value = self.request(message).await?;
        let raw = serde_json::value::to_raw_value(&value).map_err(acp::Error::into_internal_error)?;
        Ok(acp::ExtResponse::new(raw.into()))
    }

    async fn ext_notification(&self, args: acp::ExtNotification) -> acp::Result<()> {
        let message =
            sdk::UntypedMessage::new(&ext_wire_method(&args.method), &args.params).map_err(acp::Error::from)?;
        self.notify(message).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::Client as _;
    use serde_json::{Value, json};
    use std::cell::RefCell;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    #[derive(Default)]
    struct EchoAgent {
        ext_methods: RefCell<Vec<String>>,
        ext_notifications: RefCell<Vec<String>>,
    }

    #[async_trait::async_trait(?Send)]
    impl Agent for EchoAgent {
        async fn initialize(&self, args: acp::InitializeRequest) -> acp::Result<acp::InitializeResponse> {
            Ok(acp::InitializeResponse::new(args.protocol_version))
        }
        async fn authenticate(&self, _: acp::AuthenticateRequest) -> acp::Result<acp::AuthenticateResponse> {
            Ok(acp::AuthenticateResponse::default())
        }
        async fn new_session(&self, _: acp::NewSessionRequest) -> acp::Result<acp::NewSessionResponse> {
            Ok(acp::NewSessionResponse::new("s1"))
        }
        async fn prompt(&self, _: acp::PromptRequest) -> acp::Result<acp::PromptResponse> {
            Ok(acp::PromptResponse::new(acp::StopReason::EndTurn))
        }
        async fn cancel(&self, _: acp::CancelNotification) -> acp::Result<()> {
            Ok(())
        }
        async fn ext_method(&self, args: acp::ExtRequest) -> acp::Result<acp::ExtResponse> {
            self.ext_methods.borrow_mut().push(args.method.to_string());
            Ok(acp::ExtResponse::new(args.params))
        }
        async fn ext_notification(&self, args: acp::ExtNotification) -> acp::Result<()> {
            self.ext_notifications.borrow_mut().push(args.method.to_string());
            Ok(())
        }
    }

    struct Ide {
        writer: tokio::io::DuplexStream,
        reader: BufReader<tokio::io::DuplexStream>,
    }

    impl Ide {
        async fn send(&mut self, message: Value) {
            let mut line = message.to_string();
            line.push('\n');
            self.writer.write_all(line.as_bytes()).await.unwrap();
        }

        async fn recv(&mut self) -> Value {
            let mut line = String::new();
            tokio::time::timeout(std::time::Duration::from_secs(2), self.reader.read_line(&mut line))
                .await
                .expect("timed out waiting for a message from the bridge")
                .unwrap();
            serde_json::from_str(&line).unwrap()
        }
    }

    fn connect(agent: Rc<EchoAgent>) -> (Ide, AgentSideConnection, impl Future<Output = acp::Result<()>>) {
        let (ide_writer, agent_reader) = tokio::io::duplex(64 * 1024);
        let (agent_writer, ide_reader) = tokio::io::duplex(64 * 1024);
        let (client, io) = AgentSideConnection::new(
            agent,
            async_compat::Compat::new(agent_writer),
            async_compat::Compat::new(agent_reader),
        );
        let ide = Ide {
            writer: ide_writer,
            reader: BufReader::new(ide_reader),
        };
        (ide, client, io)
    }

    #[tokio::test]
    async fn passes_protocol_version_through_unchanged() {
        let local = tokio::task::LocalSet::new();
        local
            .run_until(async {
                let (mut ide, _client, io) = connect(Rc::new(EchoAgent::default()));
                tokio::task::spawn_local(io);

                ide.send(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":0}}))
                    .await;
                let response = ide.recv().await;
                assert_eq!(response["id"], 1);
                assert_eq!(response["result"]["protocolVersion"], 0);
            })
            .await;
    }

    #[tokio::test]
    async fn strips_ext_prefix_on_incoming_messages() {
        let local = tokio::task::LocalSet::new();
        local
            .run_until(async {
                let agent = Rc::new(EchoAgent::default());
                let (mut ide, _client, io) = connect(agent.clone());
                tokio::task::spawn_local(io);

                ide.send(json!({"jsonrpc":"2.0","id":7,"method":"_my/tool","params":{"x":1}}))
                    .await;
                let response = ide.recv().await;
                assert_eq!(response["id"], 7);
                assert_eq!(response["result"], json!({"x":1}));
                assert_eq!(*agent.ext_methods.borrow(), vec!["my/tool".to_string()]);

                ide.send(json!({"jsonrpc":"2.0","method":"_my/event","params":{}}))
                    .await;
                // A follow-up request makes sure the notification was processed.
                ide.send(json!({"jsonrpc":"2.0","id":8,"method":"_my/tool","params":{}}))
                    .await;
                ide.recv().await;
                assert_eq!(*agent.ext_notifications.borrow(), vec!["my/event".to_string()]);
            })
            .await;
    }

    #[tokio::test]
    async fn unsupported_method_returns_error() {
        let local = tokio::task::LocalSet::new();
        local
            .run_until(async {
                let (mut ide, _client, io) = connect(Rc::new(EchoAgent::default()));
                tokio::task::spawn_local(io);

                ide.send(json!({"jsonrpc":"2.0","id":3,"method":"session/delete","params":{"sessionId":"s1"}}))
                    .await;
                let response = ide.recv().await;
                assert_eq!(response["id"], 3);
                assert_eq!(response["error"]["code"], -32601);
            })
            .await;
    }

    #[tokio::test]
    async fn client_calls_reach_the_ide() {
        let local = tokio::task::LocalSet::new();
        local
            .run_until(async {
                let (mut ide, client, io) = connect(Rc::new(EchoAgent::default()));
                tokio::task::spawn_local(io);

                let request = tokio::task::spawn_local(async move {
                    let result = client
                        .ext_method(acp::ExtRequest::new(
                            "my/query",
                            serde_json::value::to_raw_value(&json!({"q":1})).unwrap().into(),
                        ))
                        .await
                        .unwrap();
                    client
                        .session_notification(acp::SessionNotification::new(
                            "s1",
                            acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk::new(acp::ContentBlock::Text(
                                acp::TextContent::new("hi"),
                            ))),
                        ))
                        .await
                        .unwrap();
                    serde_json::from_str::<Value>(result.0.get()).unwrap()
                });

                let outgoing = ide.recv().await;
                assert_eq!(outgoing["method"], "_my/query");
                assert_eq!(outgoing["params"], json!({"q":1}));
                ide.send(json!({"jsonrpc":"2.0","id":outgoing["id"],"result":{"answer":42}}))
                    .await;

                let notification = ide.recv().await;
                assert_eq!(notification["method"], "session/update");
                assert_eq!(notification["params"]["sessionId"], "s1");

                assert_eq!(request.await.unwrap(), json!({"answer":42}));
            })
            .await;
    }
}
