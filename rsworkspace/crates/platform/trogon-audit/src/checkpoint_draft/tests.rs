use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;

use super::*;
use crate::constants::STREAM_AUDIT;
use crate::fixtures::{CHAIN, draft_of, head_at, now, signing_key, token};

fn key_id() -> KeyId {
    KeyId::from_verifying_key(signing_key(1).verifying_key())
}

#[test]
fn the_statement_names_the_chain_the_head_the_key_and_the_time() {
    let draft = draft_of(head_at(4));
    let key = key_id();

    let statement = CheckpointStatement::decode_from_slice(&draft.statement(&key)).unwrap();

    assert_eq!(statement.writer_id, "gateway-1");
    assert_eq!(statement.chain_id, CHAIN);
    assert_eq!(statement.head_sequence, 4);
    assert_eq!(statement.head_hash, draft.head().hash().as_bytes().to_vec());
    assert_eq!(statement.audit_stream, STREAM_AUDIT);
    assert_eq!(statement.audit_stream_sequence, 14);
    assert_eq!(
        SystemTime::try_from(statement.issued_at.as_option().unwrap().clone()).unwrap(),
        now()
    );
    assert_eq!(statement.key_id, key.to_string());
    assert_eq!(statement.checkpoint_id, draft.id().to_string());
    assert_eq!(statement.component, "egress_proxy");
    assert_eq!(draft.component(), &token("egress_proxy"));
    assert_eq!(draft.issued_at(), now());
}

#[test]
fn the_signing_input_is_the_header_and_the_statement_it_stores() {
    let draft = draft_of(head_at(4));
    let key = key_id();

    let expected = format!(
        "{}.{}",
        checkpoint_jws::protected_header(&key),
        URL_SAFE_NO_PAD.encode(draft.statement(&key))
    );
    assert_eq!(draft.signing_input(&key), expected.into_bytes());
}
