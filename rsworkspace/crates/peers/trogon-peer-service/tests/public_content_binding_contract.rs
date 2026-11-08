use trogon_peer_service::test_support::Fixture;
type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;


#[tokio::test]
async fn content_ports_preserve_semantics_but_verify_storage_integrity_and_scope_independently() -> TestResult {
    use trogon_decider_runtime::{CommandAuthorizer as _, CommandPrincipal, PrincipalId, PrincipalKind};
    use trogon_peer::{PeerId, SealedContentRef};
    use trogon_peer_service::{ContentBinding, ContentOpen as _, ContentSeal as _, MessageText, PeerAccess, PeerOperation, PeerPortError};

    let f = Fixture::new().await?;
    let peer = PeerId::parse(f.a.peer_id.clone().ok_or(PeerPortError::Integrity)?)?;
    let scope = PeerAccess::new(f.alice.clone(), peer, PeerOperation::Send);
    let principal = CommandPrincipal::new(PrincipalKind::Agent, PrincipalId::new(f.alice.caller().as_str())?);
    f.policy.authorize(&principal, &scope)?;
    let text = MessageText::new("fixture stored bytes with é".into())?;
    let first = f.content.seal(&scope, &text).await?;
    let second = f.content.seal(&scope, &text).await?;
    assert_ne!(first.content().reference().as_str(), second.content().reference().as_str());
    assert_eq!(first.content().semantic_fingerprint(), second.content().semantic_fingerprint());
    let integrity = <sha2::Sha256 as sha2::Digest>::digest(text.as_str().as_bytes());
    assert_eq!(first.content().reference().integrity_digest(), &integrity[..]);
    assert_ne!(first.content().semantic_fingerprint(), first.content().reference().integrity_digest());
    assert!(f.content.open(&scope, &first).await? == Some(text.clone()));
    let mut corrupted = first.content().reference().to_claim_check();
    corrupted.digest.as_option_mut().ok_or(PeerPortError::Integrity)?.value.as_mut().ok_or(PeerPortError::Integrity)?[0] ^= 1;
    let bad_integrity = ContentBinding::from_port(f.alice.tenant().clone(), SealedContentRef::new(corrupted)?,
        first.content().size(), first.content().semantic_fingerprint().to_vec())?;
    assert!(matches!(f.content.open(&scope, &bad_integrity).await, Err(PeerPortError::Integrity)),
        "valid semantic token must not authenticate mismatched stored bytes");
    let bad_size = ContentBinding::from_port(f.alice.tenant().clone(), first.content().reference().clone(),
        first.content().size().checked_add(1).ok_or(PeerPortError::Integrity)?, first.content().semantic_fingerprint().to_vec())?;
    assert!(matches!(f.content.open(&scope, &bad_size).await, Err(PeerPortError::Integrity)),
        "matching ClaimCheck and fingerprint must not validate the wrong declared UTF-8 size");
    let mut fingerprint = first.content().semantic_fingerprint().to_vec();
    fingerprint[0] ^= 1;
    let bad_semantics = ContentBinding::from_port(f.alice.tenant().clone(), first.content().reference().clone(),
        first.content().size(), fingerprint)?;
    assert!(matches!(f.content.open(&scope, &bad_semantics).await, Err(PeerPortError::Integrity)),
        "valid storage digest must not replace the semantic binding");
    let foreign = ContentBinding::from_port(trogon_peer::TenantId::new("foreign")?, first.content().reference().clone(),
        first.content().size(), first.content().semantic_fingerprint().to_vec())?;
    assert!(matches!(f.content.open(&scope, &foreign).await, Err(PeerPortError::Denied)));
    Ok(())
}
