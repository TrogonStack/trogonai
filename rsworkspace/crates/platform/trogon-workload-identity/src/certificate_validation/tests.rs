use super::*;
use crate::test_support::TestCa;
use crate::tests::PEER_ID;

#[test]
fn a_trust_anchor_must_consume_its_entire_der_input() {
    let ca = TestCa::new();
    let identity = ca.issue(PEER_ID, Duration::from_secs(600));
    let chain = identity.certificates();
    let root = ca.root();
    let now = SystemTime::now();
    assert!(verify_chain(&chain, std::slice::from_ref(&root), now, CertificateUse::Client).is_ok());
    for suffix in [vec![0], root.to_vec()] {
        let mut ambiguous = root.to_vec();
        ambiguous.extend_from_slice(&suffix);
        assert_eq!(
            verify_chain(&chain, &[CertificateDer::from(ambiguous)], now, CertificateUse::Client),
            Err(MaterialError::Certificate)
        );
    }
}
