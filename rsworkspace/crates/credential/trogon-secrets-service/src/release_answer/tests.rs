use buffa::{Message as _, MessageField};
use grpc_nats_micro::status_codec::decode_reply;
use grpc_nats_micro::{ContentType, ReplyError, ServiceErrorCode, answer};
use nkeys::{KeyPair, XKey};
use tokio::time::Instant;
use trogon_std::log_capture::{CapturedEvents, LevelFilter};
use trogonai_proto::nats::micro::v1alpha1::{ContentType as ProtoContentType, ServiceOptions};
use trogonai_proto::secrets::exchange::v1alpha1::resolve_for_release_response::Result as Outcome;
use trogonai_proto::secrets::exchange::v1alpha1::{ResolveFailureCategory, ResolveForReleaseResponse};

use super::*;
use crate::fixtures::{RUNTIME_SERVICE, release_subject, release_wire, request, sealed_release, tenant};
use crate::scripted_release::ScriptedRelease;
use crate::{CredentialReleaseError, RELEASE_DEADLINE};

const UNDECODABLE: &[u8] = &[0xff, 0xff, 0xff];

fn protobuf_only() -> ServiceOptions {
    ServiceOptions {
        content_type: ProtoContentType::CONTENT_TYPE_PROTOBUF.into(),
        ..Default::default()
    }
}

async fn ask(subject: &str, payload: &[u8], release: &ScriptedRelease) -> Result<Outcome, ReplyError> {
    let reply = answer(None, payload, &protobuf_only(), &ReleaseAnswer::new(subject, release))
        .await
        .unwrap();
    decode_reply::<ResolveForReleaseResponse>(Some(&reply.headers), &reply.body, ContentType::Protobuf)
        .map(|response| response.result.unwrap())
}

fn category(outcome: Outcome) -> Option<ResolveFailureCategory> {
    match outcome {
        Outcome::Failure(failure) => failure.category.as_known(),
        Outcome::Access(_) => None,
    }
}

fn code(error: ReplyError) -> ServiceErrorCode {
    match error {
        ReplyError::Service(error) => error.code(),
        other => panic!("reply did not decode: {other:?}"),
    }
}

#[tokio::test]
async fn a_subject_that_names_no_caller_is_denied_before_the_payload_is_read() {
    let tenant = tenant();
    let release = ScriptedRelease::answering(Ok(sealed_release()));
    for subject in [
        "fake.subject".to_owned(),
        release_subject(&tenant).replace(tenant.as_str(), &KeyPair::new_user().public_key()),
        release_subject(&tenant).replace(".service.", ".agent."),
        release_subject(&tenant).replace(RUNTIME_SERVICE, "billing!runner"),
    ] {
        assert_eq!(
            category(ask(&subject, UNDECODABLE, &release).await.unwrap()),
            Some(ResolveFailureCategory::RESOLVE_FAILURE_CATEGORY_DENIED),
            "{subject}"
        );
    }
    assert!(release.calls().is_empty());
}

#[tokio::test]
async fn a_payload_that_does_not_decode_is_an_invalid_argument() {
    let release = ScriptedRelease::answering(Ok(sealed_release()));
    let error = ask(&release_subject(&tenant()), UNDECODABLE, &release)
        .await
        .unwrap_err();
    assert_eq!(code(error), ServiceErrorCode::INVALID_ARGUMENT);
    assert!(release.calls().is_empty());
}

#[tokio::test]
async fn a_payload_the_contract_rejects_is_an_invalid_argument() {
    let release = ScriptedRelease::answering(Ok(sealed_release()));
    let mut wire = release_wire(&XKey::new().public_key());
    wire.credential = MessageField::none();
    let error = ask(&release_subject(&tenant()), &wire.encode_to_vec(), &release)
        .await
        .unwrap_err();
    assert_eq!(code(error), ServiceErrorCode::INVALID_ARGUMENT);
    assert!(release.calls().is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_release_past_the_deadline_is_unavailable() {
    let release = ScriptedRelease::hanging();
    let payload = release_wire(&XKey::new().public_key()).encode_to_vec();
    let started = Instant::now();
    let outcome = ask(&release_subject(&tenant()), &payload, &release).await.unwrap();
    assert_eq!(
        category(outcome),
        Some(ResolveFailureCategory::RESOLVE_FAILURE_CATEGORY_UNAVAILABLE)
    );
    assert_eq!(started.elapsed(), RELEASE_DEADLINE);
}

#[tokio::test(start_paused = true)]
async fn a_release_past_the_deadline_is_logged_with_the_deadline_it_missed() {
    let events = CapturedEvents::new();
    let _capture = events.install(LevelFilter::ERROR);
    let subject = release_subject(&tenant());
    let payload = release_wire(&XKey::new().public_key()).encode_to_vec();

    ask(&subject, &payload, &ScriptedRelease::hanging()).await.unwrap();

    let event = events
        .events()
        .into_iter()
        .find(|event| event.message() == Some("release did not finish within its deadline; answered unavailable"))
        .expect("the missed deadline is logged");
    assert_eq!(event.field("subject"), Some(subject.as_str()));
    let deadline_ms = RELEASE_DEADLINE.as_millis().to_string();
    assert_eq!(event.field("deadline_ms"), Some(deadline_ms.as_str()));
}

#[tokio::test]
async fn a_refused_release_answers_its_category() {
    let release = ScriptedRelease::answering(Err(CredentialReleaseError::KeyDenied));
    let payload = release_wire(&XKey::new().public_key()).encode_to_vec();
    let outcome = ask(&release_subject(&tenant()), &payload, &release).await.unwrap();
    assert_eq!(
        category(outcome),
        Some(ResolveFailureCategory::RESOLVE_FAILURE_CATEGORY_KEY_DENIED)
    );
}

#[tokio::test]
async fn a_release_is_asked_for_the_subject_caller_and_answers_the_sealed_access() {
    let tenant = tenant();
    let xkey = XKey::new().public_key();
    let release = ScriptedRelease::answering(Ok(sealed_release()));
    let outcome = ask(
        &release_subject(&tenant),
        &release_wire(&xkey).encode_to_vec(),
        &release,
    )
    .await
    .unwrap();
    let Outcome::Access(access) = outcome else {
        panic!("a release answered a failure");
    };
    assert_eq!(access.xkey_sealed_value, sealed_release().sealed_value().as_bytes());
    assert_eq!(release.calls(), vec![(request(&tenant), xkey)]);
}
