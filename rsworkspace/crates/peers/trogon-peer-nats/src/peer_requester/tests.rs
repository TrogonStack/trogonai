#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::time::Duration;

use futures::StreamExt;
use trogon_nats::test_support::JetStreamTestServer;

use crate::{
    PeerAccount, PeerCaller, PeerConnectionId, PeerIngress, PeerMethod, PeerPrincipal, PeerReplySubject,
    PeerRequestError, PeerRequestTimeout, PeerRequestTimeoutError, PeerRequester, PeerRequesterConfig, PeerTenant,
};

fn config(account: PeerAccount, timeout: Duration) -> PeerRequesterConfig {
    PeerRequesterConfig::new(
        account,
        PeerCaller::new("caller_alpha").unwrap(),
        PeerConnectionId::new("connection_alpha").unwrap(),
        PeerRequestTimeout::new(timeout).unwrap(),
    )
}

#[test]
fn request_timeout_has_a_positive_finite_bound() {
    assert_eq!(
        PeerRequestTimeout::new(Duration::ZERO),
        Err(PeerRequestTimeoutError::Zero)
    );
    assert_eq!(
        PeerRequestTimeout::new(Duration::from_secs(3)),
        Err(PeerRequestTimeoutError::TooLong)
    );
    assert_eq!(
        PeerRequestTimeout::new(Duration::from_millis(50))
            .unwrap()
            .as_duration(),
        Duration::from_millis(50)
    );
    assert!(PeerRequestTimeout::new(PeerRequestTimeout::MAX).is_ok());
}

#[tokio::test]
async fn request_uses_an_exact_scoped_reply_and_preserves_both_route_modes() {
    let broker = JetStreamTestServer::start_with_atomic_batches().await;
    let responder = broker.client().await;
    for (account, subject) in [
        (
            PeerAccount::SharedFixture {
                tenant: PeerTenant::new("tenant_alpha").unwrap(),
            },
            "tenant_alpha.peer.v1.caller_alpha.PeerService.Enroll",
        ),
        (
            PeerAccount::TenantAccount {
                tenant: PeerTenant::new("tenant_alpha").unwrap(),
            },
            "peer.v1.caller_alpha.PeerService.Enroll",
        ),
    ] {
        let principal = PeerPrincipal::new(account.tenant().clone(), PeerCaller::new("caller_alpha").unwrap());
        let mut incoming = responder.subscribe(subject.to_owned()).await.unwrap();
        responder.flush().await.unwrap();
        let requester = PeerRequester::new(broker.client().await, config(account.clone(), Duration::from_secs(1)));
        let receive = async {
            let message = tokio::time::timeout(Duration::from_secs(2), incoming.next())
                .await
                .ok()
                .flatten()?;
            let reply = message.reply.clone()?;
            responder.publish(reply.clone(), vec![9, 8, 7].into()).await.unwrap();
            Some((message, reply))
        };
        let (response, received) = tokio::join!(requester.request(PeerMethod::Enroll, vec![0, 255, 10]), receive);
        let response = response.expect("requester must return the responder's message");
        let (message, reply) = received.expect("broker must receive the peer request");
        assert_eq!(response.payload.as_ref(), &[9, 8, 7]);
        assert_eq!(message.payload.as_ref(), &[0, 255, 10]);
        assert_eq!(message.subject.as_str(), subject);
        assert_eq!(
            PeerIngress::new(account)
                .parse(message.subject.as_str())
                .unwrap()
                .method(),
            PeerMethod::Enroll
        );
        let validated = PeerReplySubject::parse_for(&principal, reply.as_str())
            .expect("the actual broker reply must pass the strict containment parser");
        let segments: Vec<_> = validated.as_str().split('.').collect();
        assert_eq!(segments.len(), 5);
        assert_eq!(
            &segments[..4],
            &["_INBOX", "tenant_alpha", "caller_alpha", "connection_alpha"]
        );
        assert!(!segments[4].is_empty());
    }
}

#[tokio::test]
async fn separate_requests_use_distinct_nonce_subjects_on_the_same_connection() {
    let broker = JetStreamTestServer::start_with_atomic_batches().await;
    let responder = broker.client().await;
    let subject = "tenant_alpha.peer.v1.caller_alpha.PeerService.Roster";
    let mut incoming = responder.subscribe(subject).await.unwrap();
    responder.flush().await.unwrap();
    let requester = PeerRequester::new(
        broker.client().await,
        config(
            PeerAccount::SharedFixture {
                tenant: PeerTenant::new("tenant_alpha").unwrap(),
            },
            Duration::from_secs(1),
        ),
    );
    let receive = async {
        let mut replies = Vec::new();
        for _ in 0..2 {
            let message = tokio::time::timeout(Duration::from_secs(2), incoming.next())
                .await
                .ok()
                .flatten()?;
            let reply = message.reply.clone()?;
            replies.push(reply.to_string());
            responder.publish(reply, message.payload).await.unwrap();
        }
        Some(replies)
    };
    let send = async {
        let first = requester.request(PeerMethod::Roster, vec![1]).await?;
        let second = requester.request(PeerMethod::Roster, vec![2]).await?;
        Ok::<_, PeerRequestError>((first, second))
    };
    let (responses, replies) = tokio::join!(send, receive);
    let (first, second) = responses.expect("both peer requests must return responses");
    assert_eq!(first.payload.as_ref(), &[1]);
    assert_eq!(second.payload.as_ref(), &[2]);
    let replies = replies.expect("responder must observe both requests");
    assert_ne!(replies[0], replies[1]);
    let principal = PeerPrincipal::new(
        PeerTenant::new("tenant_alpha").unwrap(),
        PeerCaller::new("caller_alpha").unwrap(),
    );
    for reply in replies {
        assert!(PeerReplySubject::parse_for(&principal, &reply).is_ok());
        assert_eq!(reply.split('.').count(), 5);
    }
}

#[tokio::test]
async fn a_silent_subscriber_causes_a_typed_finite_timeout() {
    let broker = JetStreamTestServer::start_with_atomic_batches().await;
    let responder = broker.client().await;
    let _silent = responder
        .subscribe("peer.v1.caller_alpha.PeerService.Status")
        .await
        .unwrap();
    responder.flush().await.unwrap();
    let requester = PeerRequester::new(
        broker.client().await,
        config(
            PeerAccount::TenantAccount {
                tenant: PeerTenant::new("tenant_alpha").unwrap(),
            },
            Duration::from_millis(50),
        ),
    );
    let response = tokio::time::timeout(Duration::from_secs(1), requester.request(PeerMethod::Status, vec![]))
        .await
        .expect("the requester must not wait indefinitely");
    assert!(
        matches!(response,Err(PeerRequestError::Transport(error)) if error.kind()==async_nats::RequestErrorKind::TimedOut)
    );
}
