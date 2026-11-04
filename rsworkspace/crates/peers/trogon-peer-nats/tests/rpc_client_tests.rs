use buffa_types::google::protobuf::Any;
use futures::StreamExt as _;
use grpc_nats_micro::{
    ContentType, ReplyError,
    constants::{HEADER_CONTENT_TYPE, HEADER_ERROR_CODE},
};
use std::time::Duration;
use trogon_nats::test_support::JetStreamTestServer;
use trogon_peer_nats::{
    PeerAccount, PeerCaller, PeerConnectionId, PeerRequestTimeout, PeerRequester, PeerRequesterConfig, PeerRpcClient,
    PeerRpcClientError, PeerTenant,
};
use trogonai_proto::{
    google::rpc::{ErrorInfo, Status},
    peers::v1alpha1 as proto,
};

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;
async fn client(
    server: &JetStreamTestServer,
    encoding: ContentType,
) -> Result<PeerRpcClient, Box<dyn std::error::Error + Send + Sync>> {
    Ok(PeerRpcClient::new(
        PeerRequester::new(
            server.client().await,
            PeerRequesterConfig::new(
                PeerAccount::SharedFixture {
                    tenant: PeerTenant::new("tenant")?,
                },
                PeerCaller::new("alice")?,
                PeerConnectionId::new("connection")?,
                PeerRequestTimeout::new(Duration::from_secs(1))?,
            ),
        ),
        encoding,
    ))
}

macro_rules! check {
    ($client:ident, $incoming:ident, $responder:ident, $encoding:ident, $method:ident, $request:ident, $response:ident, $terminal:literal) => {{
        let serve = async {
            let message = tokio::time::timeout(Duration::from_secs(2), $incoming.next())
                .await?
                .ok_or("no request")?;
            assert_eq!(
                message.subject.as_str(),
                concat!("tenant.peer.v1.alice.PeerService.", $terminal)
            );
            assert_eq!(
                message
                    .headers
                    .as_ref()
                    .and_then(|h| h.get(HEADER_CONTENT_TYPE))
                    .map(|h| h.as_str()),
                Some($encoding.header_value())
            );
            let _: proto::$request = $encoding.decode(&message.payload)?;
            let reply = message.reply.ok_or("missing reply")?;
            assert_eq!(reply.as_str().split('.').count(), 5);
            assert!(reply.as_str().starts_with("_INBOX.tenant.alice.connection."));
            let mut headers = async_nats::HeaderMap::new();
            headers.insert(HEADER_CONTENT_TYPE, $encoding.header_value());
            $responder
                .publish_with_headers(reply, headers, $encoding.encode(&proto::$response::default())?.into())
                .await?;
            Ok::<_, Box<dyn std::error::Error + Send + Sync>>(())
        };
        let request = proto::$request::default();
        let (response, served) = tokio::join!($client.$method(&request), serve);
        assert!(response.is_ok(), "typed $client did not return a successful response");
        served?;
    }};
}
#[tokio::test]
async fn typed_operations_preserve_route_encoding_and_exact_reply() -> TestResult {
    let broker = JetStreamTestServer::start_with_atomic_batches().await;
    let responder = broker.client().await;
    let mut incoming = responder.subscribe("tenant.peer.v1.alice.PeerService.*").await?;
    responder.flush().await?;
    for encoding in [ContentType::Protobuf, ContentType::Json] {
        let client = client(&broker, encoding).await?;
        check!(
            client,
            incoming,
            responder,
            encoding,
            enroll,
            EnrollRequest,
            EnrollResponse,
            "Enroll"
        );
        check!(
            client,
            incoming,
            responder,
            encoding,
            heartbeat,
            HeartbeatRequest,
            HeartbeatResponse,
            "Heartbeat"
        );
        check!(
            client,
            incoming,
            responder,
            encoding,
            detach,
            DetachRequest,
            DetachResponse,
            "Detach"
        );
        check!(
            client,
            incoming,
            responder,
            encoding,
            roster,
            RosterRequest,
            RosterResponse,
            "Roster"
        );
        check!(
            client,
            incoming,
            responder,
            encoding,
            send,
            SendRequest,
            SendResponse,
            "Send"
        );
        check!(
            client,
            incoming,
            responder,
            encoding,
            status,
            StatusRequest,
            StatusResponse,
            "Status"
        );
        check!(
            client,
            incoming,
            responder,
            encoding,
            prepare_batch,
            PrepareBatchRequest,
            PrepareBatchResponse,
            "PrepareBatch"
        );
        check!(
            client,
            incoming,
            responder,
            encoding,
            acknowledge_batch,
            AcknowledgeBatchRequest,
            AcknowledgeBatchResponse,
            "AcknowledgeBatch"
        );
    }
    Ok(())
}

#[tokio::test]
async fn error_header_precedes_response_shape_and_preserves_complete_status_and_reply_encoding() -> TestResult {
    let broker = JetStreamTestServer::start_with_atomic_batches().await;
    let responder = broker.client().await;
    let mut incoming = responder.subscribe("tenant.peer.v1.alice.PeerService.Detach").await?;
    responder.flush().await?;
    for encoding in [ContentType::Json, ContentType::Protobuf] {
        let client = client(&broker, ContentType::Protobuf).await?;
        let detail = ErrorInfo {
            reason: "DENIED".into(),
            domain: "peer-fixture".into(),
            ..Default::default()
        };
        let status = Status {
            code: 3,
            message: "sanitized".into(),
            details: vec![Any::pack(&detail, ErrorInfo::TYPE_URL)],
        };
        let serve = async {
            let message = tokio::time::timeout(Duration::from_secs(2), incoming.next())
                .await?
                .ok_or("no request")?;
            let mut headers = async_nats::HeaderMap::new();
            headers.insert(HEADER_ERROR_CODE, "7");
            headers.insert(HEADER_CONTENT_TYPE, encoding.header_value());
            let body = encoding.encode(&status)?;
            if encoding == ContentType::Protobuf {
                let _: proto::DetachResponse = encoding.decode(&body)?;
            }
            responder
                .publish_with_headers(message.reply.ok_or("missing reply")?, headers, body.into())
                .await?;
            Ok::<_, Box<dyn std::error::Error + Send + Sync>>(())
        };
        let request = proto::DetachRequest::default();
        let (response, served) = tokio::join!(client.detach(&request), serve);
        assert!(
            matches!(&response, Err(PeerRpcClientError::Reply(ReplyError::Service(_)))),
            "error channel must take precedence"
        );
        if let Err(PeerRpcClientError::Reply(ReplyError::Service(error))) = response {
            assert_eq!(error.status().code, 7);
            assert_eq!(error.status().message, status.message);
            assert_eq!(error.status().details, status.details);
        }
        served?;
    }
    Ok(())
}
