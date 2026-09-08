use trogonai_proto::nats::micro::v1alpha1::{ContentType as ProtoContentType, ServiceOptions};

use super::{ContentType, NegotiationError};
use crate::constants::{CONTENT_TYPE_JSON, CONTENT_TYPE_PROTOBUF};
use crate::content_type_input::ContentTypeInput;

fn policy(content_type: ProtoContentType) -> ServiceOptions {
    ServiceOptions {
        content_type: content_type.into(),
        ..Default::default()
    }
}

#[test]
fn reads_the_header_values_this_binding_speaks() {
    assert_eq!(
        ContentType::from_input(&ContentTypeInput::new(CONTENT_TYPE_PROTOBUF)),
        Some(ContentType::Protobuf)
    );
    assert_eq!(
        ContentType::from_input(&ContentTypeInput::new(CONTENT_TYPE_JSON)),
        Some(ContentType::Json)
    );
    assert_eq!(ContentType::from_input(&ContentTypeInput::new("application/xml")), None);
}

#[test]
fn an_absent_header_defaults_to_protobuf_when_the_policy_allows_either() {
    let negotiated = ContentType::negotiate(&ServiceOptions::default(), None).expect("either encoding is allowed");
    assert_eq!(negotiated, ContentType::Protobuf);
}

#[test]
fn an_absent_header_takes_the_only_encoding_the_policy_allows() {
    let json = ContentType::negotiate(&policy(ProtoContentType::CONTENT_TYPE_JSON), None).expect("a json-only policy");
    assert_eq!(json, ContentType::Json);

    let protobuf =
        ContentType::negotiate(&policy(ProtoContentType::CONTENT_TYPE_PROTOBUF), None).expect("a protobuf-only policy");
    assert_eq!(protobuf, ContentType::Protobuf);
}

#[test]
fn a_header_the_policy_allows_is_accepted() {
    let requested = ContentTypeInput::new(CONTENT_TYPE_JSON);

    let unrestricted =
        ContentType::negotiate(&ServiceOptions::default(), Some(&requested)).expect("either encoding is allowed");
    assert_eq!(unrestricted, ContentType::Json);

    let restricted = ContentType::negotiate(&policy(ProtoContentType::CONTENT_TYPE_JSON), Some(&requested))
        .expect("a json-only policy");
    assert_eq!(restricted, ContentType::Json);
}

#[test]
fn a_header_outside_the_policy_is_rejected() {
    let requested = ContentTypeInput::new(CONTENT_TYPE_JSON);

    let error = ContentType::negotiate(&policy(ProtoContentType::CONTENT_TYPE_PROTOBUF), Some(&requested))
        .expect_err("a protobuf-only policy turns a json caller away");

    assert!(matches!(
        error,
        NegotiationError::NotAllowed {
            requested: ContentType::Json
        }
    ));
}

/// The rejection has to name what the caller actually sent, so an operator
/// reading it does not have to guess which header value was refused.
#[test]
fn a_header_the_binding_does_not_speak_is_retained_verbatim() {
    let requested = ContentTypeInput::new("application/xml");

    let error = ContentType::negotiate(&ServiceOptions::default(), Some(&requested))
        .expect_err("an unknown encoding is turned away");

    assert!(matches!(&error, NegotiationError::Unsupported { requested: retained } if retained == &requested));
    assert_eq!(
        error.to_string(),
        "unrecognized Content-Type header value: application/xml"
    );
}
