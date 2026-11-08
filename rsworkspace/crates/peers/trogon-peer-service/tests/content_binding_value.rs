mod support;
use support::*;
use trogon_decider_runtime::{
    CommandAuthorizer, CommandPrincipal, PrincipalId, PrincipalKind, ReadFrom, ReadStreamRequest, StreamRead,
};
use trogon_peer::{AcceptedMessage, MessageId, PeerId, PeerValueError, TenantId};
use trogon_peer_service::{ContentBinding, ContentOpen, MessageText, PeerAccess, PeerOperation, PeerPortError};
use trogonai_proto::peers::v1alpha1 as proto;

#[tokio::test]
async fn admitted_content_can_be_rebound_without_bypassing_open_scope_or_parts_validation() -> TestResult {
    let f = Fixture::new().await?;
    let peer = PeerId::parse(f.a.peer_id.clone().ok_or(PeerPortError::Integrity)?)?;
    let scope = PeerAccess::new(f.alice.clone(), peer.clone(), PeerOperation::Send);
    let principal = CommandPrincipal::new(PrincipalKind::Agent, PrincipalId::new(f.alice.caller().as_str())?);
    f.policy.authorize(&principal, &scope)?;
    let text = MessageText::new("typed content remains scoped".into())?;
    let sent = f.app().send(&f.alice, f.send("typed_binding", text.as_str())).await?;
    let message = MessageId::new(sent.message.message_id.clone().ok_or(PeerPortError::Integrity)?)?;
    let read = f
        .store
        .read_stream_bounded(
            ReadStreamRequest {
                stream_id: &peer,
                from: ReadFrom::Beginning,
            },
            4096,
        )
        .await?;
    let mut state = trogon_peer::commands::state::initial_state();
    for event in read.events {
        let event = event
            .decode::<proto::PeerEvent>()?
            .into_decoded()
            .ok_or(PeerPortError::Integrity)?;
        state = trogon_peer::commands::state::evolve(state, &event)?;
    }
    let evidence = AcceptedMessage::from_sender_history(&state, &message)?;
    let admitted = evidence.content().clone();
    let retained = ContentBinding::from_content(f.alice.tenant().clone(), admitted.clone());
    assert!(f.content.open(&scope, &retained).await? == Some(text.clone()));
    let recipient = PeerId::parse(f.b.peer_id.clone().ok_or(PeerPortError::Integrity)?)?;
    let recipient_scope = PeerAccess::new(f.bob.clone(), recipient, PeerOperation::Prepare);
    let recipient_principal = CommandPrincipal::new(PrincipalKind::Agent, PrincipalId::new(f.bob.caller().as_str())?);
    f.policy.authorize(&recipient_principal, &recipient_scope)?;
    assert!(f.content.open(&recipient_scope, &retained).await? == Some(text.clone()));
    let parts = ContentBinding::from_port(
        f.alice.tenant().clone(),
        admitted.reference().clone(),
        admitted.size(),
        admitted.semantic_fingerprint().to_vec(),
    )?;
    assert!(f.content.open(&scope, &parts).await? == Some(text));
    let foreign = ContentBinding::from_content(TenantId::new("foreign")?, admitted.clone());
    assert!(matches!(
        f.content.open(&scope, &foreign).await,
        Err(PeerPortError::Denied)
    ));
    for size in [0, 2049] {
        assert!(matches!(
            ContentBinding::from_port(
                f.alice.tenant().clone(),
                admitted.reference().clone(),
                size,
                admitted.semantic_fingerprint().to_vec()
            ),
            Err(PeerValueError::InvalidContent)
        ));
    }
    for fingerprint in [Vec::new(), vec![1; 31], vec![1; 33]] {
        assert!(matches!(
            ContentBinding::from_port(
                f.alice.tenant().clone(),
                admitted.reference().clone(),
                admitted.size(),
                fingerprint
            ),
            Err(PeerValueError::InvalidSemanticFingerprint)
        ));
    }
    Ok(())
}
