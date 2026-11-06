use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use buffa::MessageField;
use buffa_types::google::protobuf::Timestamp;
use ed25519_dalek::Signer;

use super::*;
use crate::chain_head::ChainHead;
use crate::checkpoint_draft::CheckpointDraft;
use crate::constants::STREAM_AUDIT;
use crate::fixtures::{OTHER_CHAIN, SequentialIds, chain_builder, now, signed_with, signing_key, token, valid_chain};
use crate::key_id::KeyIdError;
use crate::verify_chain_range::verify_chain_range;

const MINUTE: Duration = Duration::from_secs(60);

fn key_id(seed: u8) -> KeyId {
    KeyId::from_verifying_key(signing_key(seed).verifying_key())
}

/// The head of a three-record chain that verified.
fn verified_head() -> VerifiedChainHead {
    let mut builder = chain_builder();
    verify_chain_range(builder.genesis(), valid_chain(&mut builder, 3))
        .unwrap()
        .into_head()
}

fn draft_for(head: &VerifiedChainHead) -> CheckpointDraft {
    CheckpointDraft::new(
        CheckpointId::generate(&SequentialIds::default()),
        head.key().clone(),
        token("egress_proxy"),
        ChainHead::new(head.sequence(), head.hash(), STREAM_AUDIT, head.stream_sequence()),
        now(),
    )
}

fn trusted() -> TrustedKeys {
    TrustedKeys::new([key_id(1), key_id(2)])
}

fn bounds() -> TimeBounds {
    TimeBounds::new(now() - MINUTE, now() + MINUTE)
}

fn verify(payload: &[u8]) -> Result<VerifiedCheckpoint, CheckpointVerificationError> {
    verify_checkpoint(payload, &verified_head(), &trusted(), &bounds())
}

fn header(alg: &str, kid: &str, typ: &str) -> String {
    URL_SAFE_NO_PAD.encode(format!(r#"{{"alg":"{alg}","kid":"{kid}","typ":"{typ}"}}"#))
}

fn good_header(seed: u8) -> String {
    header("EdDSA", &key_id(seed).to_string(), "trogonai-audit-checkpoint+jws")
}

/// `statement` under `header`, signed by key `seed` exactly as given.
fn signed(seed: u8, header: &str, statement: &[u8]) -> Vec<u8> {
    let signature = signing_key(seed).sign(&checkpoint_jws::signing_input(header, statement));
    AuditCheckpoint {
        statement: statement.to_vec(),
        detached_jws: checkpoint_jws::detached(header, &signature),
    }
    .encode_to_vec()
}

/// The statement the writer would sign for the verified head, under key 1.
fn statement() -> CheckpointStatement {
    let bytes = draft_for(&verified_head()).statement(&key_id(1));
    CheckpointStatement::decode_from_slice(&bytes).unwrap()
}

/// `edit` applied to the writer's statement, then signed by key 1, so only the
/// edited field is wrong.
fn signed_statement(edit: impl FnOnce(&mut CheckpointStatement)) -> Vec<u8> {
    let mut statement = statement();
    edit(&mut statement);
    signed(1, &good_header(1), &statement.encode_to_vec())
}

fn with_jws(detached_jws: &str) -> Vec<u8> {
    AuditCheckpoint {
        statement: statement().encode_to_vec(),
        detached_jws: detached_jws.to_owned(),
    }
    .encode_to_vec()
}

#[test]
fn a_checkpoint_the_writer_signed_verifies_against_its_head() {
    let head = verified_head();
    let draft = draft_for(&head);
    let checkpoint = signed_with(1, &draft);

    let verified = verify_checkpoint(checkpoint.payload(), &head, &trusted(), &bounds()).unwrap();

    assert_eq!(verified.id(), draft.id());
    assert_eq!(verified.issued_at(), now());
    assert_eq!(verified.key(), &key_id(1));
    assert_eq!(verified.component(), &token("egress_proxy"));
    assert_eq!(verified.head(), &head);
}

#[test]
fn refuses_what_is_not_an_audit_checkpoint() {
    assert!(matches!(
        verify(&[0xff]),
        Err(CheckpointVerificationError::Malformed(_))
    ));
}

#[test]
fn refuses_a_jws_that_is_not_detached() {
    let header = good_header(1);
    assert_eq!(
        verify(&with_jws(&format!("{header}.payload.signature"))),
        Err(CheckpointVerificationError::MalformedJws)
    );
}

#[test]
fn refuses_a_header_it_cannot_read() {
    assert_eq!(
        verify(&with_jws("not base64!..c2ln")),
        Err(CheckpointVerificationError::MalformedHeader)
    );
}

#[test]
fn refuses_another_type_or_algorithm() {
    let kid = key_id(1).to_string();
    let statement = statement().encode_to_vec();

    let jwt = header("EdDSA", &kid, "JWT");
    assert_eq!(
        verify(&signed(1, &jwt, &statement)),
        Err(CheckpointVerificationError::WrongType)
    );
    let none = header("none", &kid, "trogonai-audit-checkpoint+jws");
    assert_eq!(
        verify(&signed(1, &none, &statement)),
        Err(CheckpointVerificationError::AlgorithmNotAllowed)
    );
}

#[test]
fn refuses_a_key_id_that_is_not_an_ed25519_did_key() {
    let header = header("EdDSA", "did:web:example.com", "trogonai-audit-checkpoint+jws");
    assert_eq!(
        verify(&signed(1, &header, &statement().encode_to_vec())),
        Err(CheckpointVerificationError::MalformedKeyId(KeyIdError::NotDidKey))
    );
}

#[test]
fn refuses_a_key_that_is_not_trusted_even_when_it_signed_correctly() {
    let head = verified_head();
    let checkpoint = signed_with(3, &draft_for(&head));
    assert_eq!(
        verify_checkpoint(checkpoint.payload(), &head, &trusted(), &bounds()),
        Err(CheckpointVerificationError::UntrustedKey {
            key_id: Box::new(key_id(3))
        })
    );
}

#[test]
fn refuses_a_signature_segment_that_is_not_an_ed25519_signature() {
    let header = good_header(1);
    let short = URL_SAFE_NO_PAD.encode([0; 10]);
    for signature in [short.as_str(), "not base64!"] {
        assert_eq!(
            verify(&with_jws(&format!("{header}..{signature}"))),
            Err(CheckpointVerificationError::MalformedSignature),
            "{signature}"
        );
    }
}

#[test]
fn refuses_a_statement_edited_after_signing() {
    let head = verified_head();
    let checkpoint = signed_with(1, &draft_for(&head));
    let mut message = AuditCheckpoint::decode_from_slice(checkpoint.payload()).unwrap();
    let last = message.statement.len() - 1;
    message.statement[last] ^= 1;

    assert_eq!(
        verify_checkpoint(&message.encode_to_vec(), &head, &trusted(), &bounds()),
        Err(CheckpointVerificationError::BadSignature)
    );
}

#[test]
fn the_signature_covers_the_stored_header_and_not_a_re_encoding() {
    let head = verified_head();
    let checkpoint = signed_with(1, &draft_for(&head));
    let mut message = AuditCheckpoint::decode_from_slice(checkpoint.payload()).unwrap();
    let (_, signature) = checkpoint_jws::split(&message.detached_jws).unwrap();
    let spaced = URL_SAFE_NO_PAD.encode(format!(
        r#"{{ "alg":"EdDSA","kid":"{}","typ":"trogonai-audit-checkpoint+jws"}}"#,
        key_id(1)
    ));
    message.detached_jws = format!("{spaced}..{signature}");

    assert_eq!(
        verify_checkpoint(&message.encode_to_vec(), &head, &trusted(), &bounds()),
        Err(CheckpointVerificationError::BadSignature)
    );
}

#[test]
fn refuses_signed_bytes_that_are_not_a_statement() {
    assert!(matches!(
        verify(&signed(1, &good_header(1), &[0xff])),
        Err(CheckpointVerificationError::MalformedStatement(_))
    ));
}

#[test]
fn refuses_a_statement_naming_a_key_other_than_its_signer() {
    let payload = signed_statement(|statement| statement.key_id = key_id(2).to_string());
    assert_eq!(verify(&payload), Err(CheckpointVerificationError::KeyIdMismatch));
}

#[test]
fn refuses_a_checkpoint_of_another_writer_or_chain() {
    let head = verified_head();

    let other_writer = signed_statement(|statement| statement.writer_id = "gateway-2".to_owned());
    assert_eq!(
        verify(&other_writer),
        Err(CheckpointVerificationError::WrongWriter {
            expected: head.writer().clone(),
        })
    );
    let bad_chain = signed_statement(|statement| statement.chain_id = "not-a-chain".to_owned());
    assert_eq!(verify(&bad_chain), Err(CheckpointVerificationError::InvalidChainId));
    let other_chain = signed_statement(|statement| statement.chain_id = OTHER_CHAIN.to_owned());
    assert_eq!(
        verify(&other_chain),
        Err(CheckpointVerificationError::WrongChain {
            expected: head.chain(),
            observed: ChainId::parse(OTHER_CHAIN).unwrap(),
        })
    );
}

#[test]
fn refuses_a_checkpoint_of_another_head() {
    let earlier = signed_statement(|statement| statement.head_sequence = 2);
    assert_eq!(
        verify(&earlier),
        Err(CheckpointVerificationError::HeadSequenceMismatch {
            expected: 3,
            observed: 2,
        })
    );
    let other_hash = signed_statement(|statement| statement.head_hash = vec![0; 32]);
    assert_eq!(
        verify(&other_hash),
        Err(CheckpointVerificationError::HeadHashMismatch { sequence: 3 })
    );
    let other_position = signed_statement(|statement| statement.audit_stream_sequence = 9);
    assert_eq!(
        verify(&other_position),
        Err(CheckpointVerificationError::StreamSequenceMismatch {
            expected: 3,
            observed: 9,
        })
    );
}

#[test]
fn refuses_a_missing_or_impossible_issue_time() {
    let missing = signed_statement(|statement| statement.issued_at = MessageField::none());
    assert_eq!(verify(&missing), Err(CheckpointVerificationError::InvalidIssuedAt));
    let impossible = signed_statement(|statement| {
        statement.issued_at = MessageField::some(Timestamp {
            seconds: 1,
            nanos: -1,
            ..Timestamp::default()
        });
    });
    assert_eq!(verify(&impossible), Err(CheckpointVerificationError::InvalidIssuedAt));
}

#[test]
fn refuses_a_checkpoint_issued_outside_the_window() {
    for issued_at in [now() - MINUTE * 2, now() + MINUTE * 2] {
        let payload =
            signed_statement(|statement| statement.issued_at = MessageField::some(Timestamp::from(issued_at)));
        assert_eq!(verify(&payload), Err(CheckpointVerificationError::IssuedOutsideBounds));
    }
}

#[test]
fn refuses_an_invalid_checkpoint_id_or_component() {
    let bad_id = signed_statement(|statement| statement.checkpoint_id = "not-a-uuid".to_owned());
    assert_eq!(verify(&bad_id), Err(CheckpointVerificationError::InvalidCheckpointId));
    let bad_component = signed_statement(|statement| statement.component = "egress.proxy".to_owned());
    assert_eq!(
        verify(&bad_component),
        Err(CheckpointVerificationError::InvalidComponent)
    );
}
