use ed25519_dalek::Signer;

use super::*;
use crate::fixtures::{CHAIN, chain_key, draft_of, head_at, signed_with, signing_key};

#[test]
fn stores_the_signed_statement_and_its_detached_jws() {
    let draft = draft_of(head_at(4));
    let checkpoint = signed_with(1, &draft);
    let key = KeyId::from_verifying_key(signing_key(1).verifying_key());

    let message = AuditCheckpoint::decode_from_slice(checkpoint.payload()).unwrap();

    assert_eq!(message.statement, draft.statement(&key));
    let (header, signature) = checkpoint_jws::split(&message.detached_jws).unwrap();
    assert_eq!(header, checkpoint_jws::protected_header(&key));
    let signature = ed25519_dalek::Signature::from_slice(&checkpoint_jws::decode_segment(signature).unwrap()).unwrap();
    key.verifying_key()
        .verify_strict(&checkpoint_jws::signing_input(header, &message.statement), &signature)
        .unwrap();
    assert_eq!(checkpoint.key(), &key);
    assert_eq!(checkpoint.id(), draft.id());
    assert_eq!(checkpoint.chain(), &chain_key());
    assert_eq!(checkpoint.head_sequence(), 4);
}

#[test]
fn is_filed_under_the_component_and_names_its_chain_in_the_headers() {
    let draft = draft_of(head_at(4));
    let checkpoint = signed_with(1, &draft);
    let headers = checkpoint.headers();

    assert_eq!(checkpoint.subject(), "platform.v1.audit.checkpoint.egress_proxy.signed");
    assert_eq!(headers.get(NATS_MESSAGE_ID).unwrap().as_str(), draft.id().to_string());
    assert_eq!(headers.get(HEADER_WRITER).unwrap().as_str(), "gateway-1");
    assert_eq!(headers.get(HEADER_SEQUENCE).unwrap().as_str(), "4");
    assert_eq!(headers.get(HEADER_CHAIN).unwrap().as_str(), CHAIN);
}

#[test]
fn refuses_a_signature_its_key_id_does_not_verify() {
    let draft = draft_of(head_at(4));
    let claimed = KeyId::from_verifying_key(signing_key(1).verifying_key());
    let forged = signing_key(2).sign(&draft.signing_input(&claimed));

    assert_eq!(
        SignedCheckpoint::assemble(&draft, &CheckpointSignature::new(claimed, forged)),
        Err(CheckpointSignError::InvalidSignature)
    );
}
