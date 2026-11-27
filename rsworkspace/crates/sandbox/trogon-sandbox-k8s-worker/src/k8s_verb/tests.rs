use super::*;

#[test]
fn every_verb_has_its_own_token() {
    let tokens: std::collections::BTreeSet<_> = K8sVerb::ALL.iter().map(|verb| verb.token()).collect();
    assert_eq!(tokens.len(), K8sVerb::ALL.len());
    assert!(tokens.contains("ssh_attach") && tokens.contains("ssh_detach"));
}
