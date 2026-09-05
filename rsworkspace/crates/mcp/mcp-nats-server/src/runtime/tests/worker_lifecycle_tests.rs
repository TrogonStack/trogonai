use rmcp::model::{CustomResult, ServerNotification};
use serde_json::{Value, json};

use super::*;

fn proxy(nats: trogon_nats::AdvancedMockNatsClient) -> McpNatsProxyService<trogon_nats::AdvancedMockNatsClient> {
    McpNatsProxyService::new(
        nats,
        mcp_config().with_operation_timeout(Duration::from_secs(3)),
        McpPeerId::new("http-test").unwrap(),
        McpPeerId::new("default").unwrap(),
    )
}

fn ping_response(nats: &trogon_nats::AdvancedMockNatsClient, id: i64) {
    let encoded = wire::encode_tx::<RoleServer>(&ServerJsonRpcMessage::response(
        ServerResult::empty(()),
        RequestId::Number(id),
    ))
    .unwrap();
    nats.set_response_wire("mcp.v1.server.default.ping", encoded.headers, encoded.body);
}

fn remote_message(method: &str, reply: Option<&str>, message: ServerJsonRpcMessage) -> Message {
    let encoded = wire::encode_tx::<RoleServer>(&message).unwrap();
    Message {
        subject: format!(
            "mcp.v1.client.http-test.{}",
            mcp_nats::nats::subjects::method_suffix(method).unwrap()
        )
        .into(),
        reply: reply.map(|value| value.to_owned().into()),
        payload: encoded.body,
        headers: Some(encoded.headers),
        length: 0,
        status: None,
        description: None,
    }
}

async fn published(nats: &trogon_nats::AdvancedMockNatsClient, count: usize) -> Vec<Value> {
    tokio::time::timeout(Duration::from_secs(3), async {
        while nats.published_payloads().len() < count {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    nats.published_payloads()
        .iter()
        .map(|body| serde_json::from_slice(body).unwrap())
        .collect()
}

#[tokio::test]
async fn subscription_failure_rejects_each_subsequent_request() {
    let service = proxy(trogon_nats::AdvancedMockNatsClient::new());
    let (_http, io) = tokio::io::duplex(1024);
    let running = rmcp::service::serve_directly(NoopServerHandler, io, None);
    for id in [1, 2] {
        let error = service
            .ping(RequestContext::new(RequestId::Number(id), running.peer().clone()))
            .await
            .unwrap_err();
        assert!(error.message.contains("subscribe"), "{error}");
    }
    running.cancel().await.unwrap();
}

#[tokio::test]
async fn request_transport_failure_reaches_the_waiting_http_caller() {
    let nats = trogon_nats::AdvancedMockNatsClient::new();
    let _inbound = nats.inject_messages();
    nats.fail_next_request();
    let service = proxy(nats);
    let (_http, io) = tokio::io::duplex(1024);
    let running = rmcp::service::serve_directly(NoopServerHandler, io, None);
    let error = service
        .ping(RequestContext::new(RequestId::Number(1), running.peer().clone()))
        .await
        .unwrap_err();
    assert!(error.message.contains("request"), "{error}");
    running.cancel().await.unwrap();
}

#[tokio::test]
async fn closing_subscription_fails_pending_requests_and_disables_the_proxy() {
    let nats = trogon_nats::AdvancedMockNatsClient::new();
    let inbound = nats.inject_messages();
    ping_response(&nats, 99);
    let service = proxy(nats.clone());
    let (_http, io) = tokio::io::duplex(1024);
    let running = rmcp::service::serve_directly(NoopServerHandler, io, None);
    let (result, ()) = tokio::join!(
        service.ping(RequestContext::new(RequestId::Number(1), running.peer().clone())),
        async {
            tokio::time::timeout(Duration::from_secs(2), async {
                while nats.requested_payloads().is_empty() {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            drop(inbound);
        }
    );
    assert_eq!(result.unwrap_err().message, "MCP NATS transport closed");
    let error = service
        .ping(RequestContext::new(RequestId::Number(2), running.peer().clone()))
        .await
        .unwrap_err();
    assert_eq!(error.message, "MCP NATS proxy is unavailable");
    running.cancel().await.unwrap();
}

struct CallbackClient(mpsc::UnboundedSender<Value>);

impl rmcp::ClientHandler for CallbackClient {
    async fn on_custom_request(
        &self,
        request: CustomRequest,
        _context: RequestContext<RoleClient>,
    ) -> Result<CustomResult, ErrorData> {
        self.0.send(serde_json::to_value(&request).unwrap()).unwrap();
        if request.params == Some(json!({"deny": true})) {
            Err(ErrorData::invalid_params("callback rejected", None))
        } else {
            Ok(CustomResult::new(json!({"accepted": true})))
        }
    }

    async fn on_custom_notification(
        &self,
        notification: CustomNotification,
        _context: NotificationContext<RoleClient>,
    ) {
        self.0.send(serde_json::to_value(notification).unwrap()).unwrap();
    }
}

#[tokio::test]
async fn server_callbacks_preserve_reply_inboxes_and_http_client_outcomes() {
    let nats = trogon_nats::AdvancedMockNatsClient::new();
    let inbound = nats.inject_messages();
    ping_response(&nats, 1);
    let service = proxy(nats.clone());
    inbound
        .unbounded_send(remote_message(
            "example/query",
            Some("_INBOX.absent"),
            ServerJsonRpcMessage::request(
                ServerRequest::CustomRequest(CustomRequest::new("example/query", Some(json!({})))),
                RequestId::Number(10),
            ),
        ))
        .unwrap();
    let replies = published(&nats, 1).await;
    assert_eq!(replies[0]["id"], 10);
    assert_eq!(replies[0]["error"]["message"], "MCP HTTP client is not available");

    let (client_io, server_io) = tokio::io::duplex(16384);
    let server = rmcp::service::serve_directly(NoopServerHandler, server_io, None);
    let (events_tx, mut events) = mpsc::unbounded_channel();
    let client =
        rmcp::service::serve_directly(CallbackClient(events_tx), client_io, Some(ServerInfo::default().into()));
    service
        .ping(RequestContext::new(RequestId::Number(1), server.peer().clone()))
        .await
        .unwrap();
    for (index, deny) in [false, true].into_iter().enumerate() {
        let id = 11 + index as i64;
        let inbox = format!("_INBOX.callback.{id}");
        inbound
            .unbounded_send(remote_message(
                "example/query",
                Some(&inbox),
                ServerJsonRpcMessage::request(
                    ServerRequest::CustomRequest(CustomRequest::new("example/query", Some(json!({"deny": deny})))),
                    RequestId::Number(id),
                ),
            ))
            .unwrap();
        let event = tokio::time::timeout(Duration::from_secs(3), events.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(event["method"], "example/query");
        assert_eq!(event["params"], json!({"deny": deny}));
        let replies = published(&nats, index + 2).await;
        let reply = &replies[index + 1];
        assert_eq!(reply["id"], id);
        if deny {
            assert!(
                reply["error"]["message"]
                    .as_str()
                    .unwrap()
                    .contains("callback rejected")
            );
        } else {
            assert_eq!(reply["result"], json!({"accepted": true}));
        }
        assert_eq!(nats.published_messages()[index + 1], inbox);
    }
    inbound
        .unbounded_send(remote_message(
            "example/changed",
            None,
            ServerJsonRpcMessage::notification(ServerNotification::CustomNotification(CustomNotification::new(
                "example/changed",
                Some(json!({"revision": 7})),
            ))),
        ))
        .unwrap();
    let event = tokio::time::timeout(Duration::from_secs(3), events.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        event,
        json!({"method": "example/changed", "params": {"revision": 7, "_meta": {}}})
    );
    drop(service);
    client.cancel().await.unwrap();
    server.cancel().await.unwrap();
}
