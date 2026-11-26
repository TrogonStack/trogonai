use super::*;
use crate::fixtures::{draft_of, head_at, signing_key};

#[tokio::test]
async fn signs_the_drafts_signing_input_under_its_own_key_id() {
    let signer = LocalCheckpointSigner::new(signing_key(1));
    let draft = draft_of(head_at(4));

    let signature = signer.sign(&draft).await.unwrap();

    assert_eq!(signature.key(), &signer.key_id());
    assert_eq!(
        signer.key_id(),
        KeyId::from_verifying_key(signing_key(1).verifying_key())
    );
    signer
        .key_id()
        .verifying_key()
        .verify_strict(&draft.signing_input(&signer.key_id()), signature.signature())
        .unwrap();
}

#[test]
fn debug_shows_the_key_id_and_never_the_key() {
    let signer = LocalCheckpointSigner::new(signing_key(1));
    let debug = format!("{signer:?}");

    assert_eq!(
        debug,
        format!("LocalCheckpointSigner {{ key_id: {}, .. }}", signer.key_id())
    );
    let secret = signing_key(1).to_bytes();
    assert!(!debug.contains(&format!("{secret:?}")));
    assert!(!debug.contains(&format!("{:?}", &secret[..4])));
}
