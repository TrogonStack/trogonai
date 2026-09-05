use serde_json::json;

use super::forwarding_tests::proxy_endpoint;
use super::*;

#[tokio::test]
async fn custom_requests_preserve_parameters_metadata_and_remote_result() {
    let (_client, server) = tokio::io::duplex(1024);
    let running = rmcp::service::serve_directly(NoopServerHandler, server, None);
    for reply in [
        Ok(ServerResult::CustomResult(CustomResult::new(
            json!({"custom": "result"}),
        ))),
        Ok(ServerResult::empty(())),
        Err(ErrorData::invalid_params("custom rejection", None)),
    ] {
        let (proxy, mut commands) = proxy_endpoint();
        let mut context = RequestContext::new(RequestId::Number(17), running.peer().clone());
        context.meta.insert("test.marker".to_owned(), json!("preserved"));
        let expected = reply.clone();
        let (result, ()) = tokio::join!(
            proxy.on_custom_request(
                CustomRequest::new("example/query", Some(json!({"key": "value"}))),
                context
            ),
            async {
                let ProxyCommand::Request {
                    request,
                    request_id,
                    response_tx,
                    ..
                } = commands.recv().await.unwrap()
                else {
                    panic!("expected custom request");
                };
                assert_eq!(request_id, RequestId::Number(17));
                assert_eq!(
                    serde_json::to_value(request).unwrap(),
                    json!({
                        "method": "example/query", "params": {"key": "value", "_meta": {"test.marker": "preserved"}}
                    })
                );
                response_tx.send(reply).unwrap();
            },
        );
        match expected {
            Ok(ServerResult::CustomResult(expected)) => assert_eq!(result.unwrap(), expected),
            Ok(_) => assert!(result.unwrap_err().message.contains("custom request")),
            Err(expected) => assert_eq!(result.unwrap_err(), expected),
        }
    }
}

#[tokio::test]
async fn legacy_subscription_and_logging_requests_forward_through_sdk_dispatch() {
    let (_client, server) = tokio::io::duplex(1024);
    let running = rmcp::service::serve_directly(NoopServerHandler, server, None);
    for request in [
        json!({"method": "logging/setLevel", "params": {"level": "warning"}}),
        json!({"method": "resources/subscribe", "params": {"uri": "test://resource"}}),
        json!({"method": "resources/unsubscribe", "params": {"uri": "test://resource"}}),
    ] {
        for reply in [
            Ok(ServerResult::empty(())),
            Ok(ServerResult::CustomResult(CustomResult::new(
                json!({"unexpected": true}),
            ))),
        ] {
            let (proxy, mut commands) = proxy_endpoint();
            let parsed: ClientRequest = serde_json::from_value(request.clone()).unwrap();
            let expected_reply = reply.clone();
            let (result, ()) = tokio::join!(
                rmcp::service::Service::handle_request(
                    &proxy,
                    parsed,
                    RequestContext::new(RequestId::Number(17), running.peer().clone())
                ),
                async {
                    let ProxyCommand::Request {
                        request: forwarded,
                        response_tx,
                        ..
                    } = commands.recv().await.unwrap()
                    else {
                        panic!("expected legacy request");
                    };
                    assert_eq!(serde_json::to_value(forwarded).unwrap(), request);
                    response_tx.send(reply).unwrap();
                },
            );
            match expected_reply {
                Ok(ServerResult::EmptyResult(_)) => assert!(result.is_ok()),
                _ => assert!(
                    result
                        .unwrap_err()
                        .message
                        .contains(request["method"].as_str().unwrap())
                ),
            }
        }
    }
}

#[tokio::test]
async fn client_notifications_preserve_parameters_and_metadata_through_sdk_contexts() {
    let (client_io, server_io) = tokio::io::duplex(16384);
    let (proxy, mut commands) = proxy_endpoint();
    let server = rmcp::service::serve_directly(proxy, server_io, None);
    let client = rmcp::service::serve_directly((), client_io, Some(ServerInfo::default().into()));
    for notification in [
        json!({"method": "notifications/progress", "params": {"progressToken": "work", "progress": 2.0, "total": 4.0}}),
        json!({"method": "notifications/initialized"}),
        json!({"method": "notifications/roots/list_changed"}),
        json!({"method": "example/changed", "params": {"key": "value"}}),
    ] {
        let mut expected = notification;
        if expected.get("params").is_none() {
            expected["params"] = json!({});
        }
        expected["params"]["_meta"] = json!({"test.marker": "preserved"});
        let parsed: ClientNotification = serde_json::from_value(expected.clone()).unwrap();
        client.peer().send_notification(parsed).await.unwrap();
        let command = tokio::time::timeout(Duration::from_secs(2), commands.recv())
            .await
            .unwrap()
            .unwrap();
        let ProxyCommand::Notification {
            notification,
            response_tx,
            ..
        } = command
        else {
            panic!("expected forwarded notification");
        };
        assert_eq!(serde_json::to_value(notification).unwrap(), expected);
        response_tx.send(Ok(())).unwrap();
    }
    client.cancel().await.unwrap();
    server.cancel().await.unwrap();
}
