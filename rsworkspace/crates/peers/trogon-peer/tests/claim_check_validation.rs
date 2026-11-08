#![allow(clippy::unwrap_used, clippy::panic)]

mod support;

use support::{enrolled, run, send};
use trogon_peer::{EvolveError, commands};
use trogonai_proto::peers::v1alpha1 as proto;

#[derive(Clone, Copy, Debug)]
enum MalformedReference {
    MissingReference,
    MissingStorage,
    MissingLocator,
    EmptyLocator,
    InvalidLocator,
    MissingDigest,
    MissingAlgorithm,
    EmptyAlgorithm,
    UnknownAlgorithm,
    MissingBytes,
    EmptyBytes,
    ShortBytes,
    LongBytes,
}

impl MalformedReference {
    fn apply(self, message: &mut proto::MessageMetadata) -> Result<(), &'static str> {
        if matches!(self, Self::MissingReference) {
            message.content_ref = Default::default();
            return Ok(());
        }
        let reference = message
            .content_ref
            .as_option_mut()
            .ok_or("baseline ClaimCheck absent")?;
        match self {
            Self::MissingReference => {}
            Self::MissingStorage => reference.storage_ref = Default::default(),
            Self::MissingLocator => {
                reference
                    .storage_ref
                    .as_option_mut()
                    .ok_or("baseline locator absent")?
                    .value = None
            }
            Self::EmptyLocator => {
                reference
                    .storage_ref
                    .as_option_mut()
                    .ok_or("baseline locator absent")?
                    .value = Some(String::new())
            }
            Self::InvalidLocator => {
                reference
                    .storage_ref
                    .as_option_mut()
                    .ok_or("baseline locator absent")?
                    .value = Some("contains space".into())
            }
            Self::MissingDigest => reference.digest = Default::default(),
            Self::MissingAlgorithm => {
                reference
                    .digest
                    .as_option_mut()
                    .ok_or("baseline digest absent")?
                    .algorithm = None
            }
            Self::EmptyAlgorithm => {
                reference
                    .digest
                    .as_option_mut()
                    .ok_or("baseline digest absent")?
                    .algorithm = Some(String::new())
            }
            Self::UnknownAlgorithm => {
                reference
                    .digest
                    .as_option_mut()
                    .ok_or("baseline digest absent")?
                    .algorithm = Some("not-an-admitted-content-algorithm".into())
            }
            Self::MissingBytes => reference.digest.as_option_mut().ok_or("baseline digest absent")?.value = None,
            Self::EmptyBytes => {
                reference.digest.as_option_mut().ok_or("baseline digest absent")?.value = Some(Vec::new())
            }
            Self::ShortBytes => {
                reference.digest.as_option_mut().ok_or("baseline digest absent")?.value = Some(vec![1; 31])
            }
            Self::LongBytes => {
                reference.digest.as_option_mut().ok_or("baseline digest absent")?.value = Some(vec![1; 33])
            }
        }
        Ok(())
    }
}

#[test]
fn acceptance_persists_a_complete_claim_check_without_assigning_its_algorithm_in_the_test()
-> Result<(), Box<dyn std::error::Error>> {
    let accepted = run(enrolled("sender", 0), &send(0, 1, 3));
    let message = accepted
        .outbox
        .first()
        .ok_or("accepted entry absent")?
        .message
        .as_option()
        .ok_or("metadata absent")?;
    let reference = message.content_ref.as_option().ok_or("ClaimCheck absent")?;
    assert!(
        reference
            .storage_ref
            .value
            .as_deref()
            .is_some_and(|value| !value.is_empty())
    );
    assert!(
        reference
            .digest
            .algorithm
            .as_deref()
            .is_some_and(|value| !value.is_empty())
    );
    assert_eq!(reference.digest.value.as_deref().map(<[u8]>::len), Some(32));
    Ok(())
}

#[test]
fn malformed_claim_checks_fail_as_typed_field_admission_before_semantic_replay()
-> Result<(), Box<dyn std::error::Error>> {
    let accepted = run(enrolled("sender", 0), &send(0, 1, 3));
    for invalid in [
        MalformedReference::MissingReference,
        MalformedReference::MissingStorage,
        MalformedReference::MissingLocator,
        MalformedReference::EmptyLocator,
        MalformedReference::InvalidLocator,
        MalformedReference::MissingDigest,
        MalformedReference::MissingAlgorithm,
        MalformedReference::EmptyAlgorithm,
        MalformedReference::UnknownAlgorithm,
        MalformedReference::MissingBytes,
        MalformedReference::EmptyBytes,
        MalformedReference::ShortBytes,
        MalformedReference::LongBytes,
    ] {
        let mut entry = accepted.outbox.first().ok_or("accepted entry absent")?.clone();
        invalid.apply(entry.message.as_option_mut().ok_or("metadata absent")?)?;
        let replay = commands::state::evolve(
            enrolled("sender", 0),
            &proto::PeerEvent {
                event: Some(proto::PeerMessageAccepted { entry: entry.into() }.into()),
            },
        );
        assert!(
            matches!(
                replay,
                Err(EvolveError::MissingField { .. } | EvolveError::InvalidValue { .. })
            ),
            "{invalid:?} did not fail typed field admission: {replay:?}"
        );
    }
    Ok(())
}
