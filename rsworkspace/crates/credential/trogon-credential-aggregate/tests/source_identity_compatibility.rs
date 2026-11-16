#![allow(clippy::expect_used, clippy::unwrap_used)]

use buffa::Message as _;
use trogon_credential_aggregate::proto::{decode_identity, identity_to_proto};
use trogon_credential_domain::{
    CredentialIdentity, CredentialKind, CredentialOwnerId, CredentialScope, SourceIntegrationId, SourceKind,
};
use trogonai_proto::gateway::credentials::v1;

#[test]
fn credential_source_wire_numbers_survive_the_canonical_extraction() {
    for (source, expected_wire_value) in [
        (SourceKind::Discord, 1),
        (SourceKind::GitHub, 2),
        (SourceKind::Gitlab, 3),
        (SourceKind::Incidentio, 4),
        (SourceKind::Linear, 5),
        (SourceKind::MicrosoftGraph, 6),
        (SourceKind::Notion, 7),
        (SourceKind::Sentry, 8),
        (SourceKind::Slack, 9),
        (SourceKind::Telegram, 10),
        (SourceKind::Twitter, 11),
        (SourceKind::Datadog, 12),
        (SourceKind::GooglePubSub, 13),
        (SourceKind::Zoom, 14),
        (SourceKind::Plaid, 15),
        (SourceKind::Webhook, 16),
    ] {
        let identity = CredentialIdentity::derive(
            CredentialScope::source(CredentialOwnerId::new("project-a").unwrap(), source),
            CredentialKind::WebhookSecret,
        );
        let wire = identity_to_proto(&identity);
        assert_eq!(wire.source.unwrap().to_i32(), expected_wire_value);
        let decoded = v1::CredentialIdentity::decode_from_slice(&wire.encode_to_vec()).unwrap();
        assert_eq!(decode_identity("identity", &decoded), Ok(identity));
    }
}

#[test]
fn webhook_endpoint_scope_round_trips_through_the_canonical_credential_identity() {
    let endpoint_id = "67b82e3f-5c99-486b-b8a0-acdad9548e3d";
    let identity = CredentialIdentity::derive(
        CredentialScope::integration(
            CredentialOwnerId::new("project-a").unwrap(),
            SourceKind::Webhook,
            SourceIntegrationId::new(endpoint_id).unwrap(),
        ),
        CredentialKind::WebhookSecret,
    );
    let wire = identity_to_proto(&identity);
    assert_eq!(
        wire.source,
        Some(v1::CredentialSource::CREDENTIAL_SOURCE_WEBHOOK.into())
    );
    assert_eq!(wire.scope_key, format!("webhook/{endpoint_id}"));
    let decoded = v1::CredentialIdentity::decode_from_slice(&wire.encode_to_vec()).unwrap();
    assert_eq!(decode_identity("identity", &decoded), Ok(identity));
}
