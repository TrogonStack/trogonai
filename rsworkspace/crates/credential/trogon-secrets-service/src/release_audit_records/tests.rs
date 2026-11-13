use buffa::EnumValue;
use trogon_audit::{Obligation, Stamper};
use trogon_credential_domain::{
    AuditCorrelationId, CredentialIdentity, CredentialKey, CredentialKind, CredentialOwnerId, CredentialScope,
    InjectionLocation, RuntimeReleaseDeniedError, RuntimeServiceId, SourceKind,
};
use trogon_std::time::SystemClock;
use trogon_std::uuid::UuidV7Generator;
use trogonai_proto::platform::audit::v1::audit_actor::{Kind, ServiceActor};
use trogonai_proto::platform::audit::v1::audit_record::Detail;
use trogonai_proto::platform::audit::v1::{AuditDecision, CredentialUseKind, GatewaySurface};

use super::*;
use crate::fixtures::{CORRELATION_ID, OWNER, RUNTIME_SERVICE, identity, request, tenant};
use crate::{DenialReason, ReleaseRequest, RequesterXkey};

fn stamp() -> Stamp {
    Stamper::new(SystemClock, UuidV7Generator).stamp()
}

fn release_of(request: ReleaseRequest) -> ReleaseRecord {
    let recipient = RequesterXkey::parse(&nkeys::XKey::new().public_key()).unwrap();
    ReleaseRecord::new(
        request,
        trogon_credential_domain::CredentialVersion::new(1).unwrap(),
        recipient,
        std::time::SystemTime::UNIX_EPOCH,
    )
}

/// The bound the release contract puts on a correlation id, an owner id and a
/// location name, and the domain on a runtime service id.
const CONTRACT_MAX_LEN: usize = 128;

fn widest(prefix: &str, fill: char) -> String {
    format!("{prefix}{}", fill.to_string().repeat(CONTRACT_MAX_LEN - prefix.len()))
}

fn credential_name(key: &CredentialKey) -> String {
    format!("projects/{OWNER}/credentials/{}", key.credential_id())
}

fn service_actor() -> Kind {
    ServiceActor {
        service_id: RUNTIME_SERVICE.to_owned(),
    }
    .into()
}

#[test]
fn the_widest_names_a_release_request_holds_are_recordable_as_a_release_and_a_denial() {
    let owner = CredentialOwnerId::new(widest("org.team:alpha_1-", 'o')).unwrap();
    let credential = CredentialIdentity::derive(
        CredentialScope::source(owner, SourceKind::GitHub),
        CredentialKind::WebhookSecret,
    )
    .key();
    let runtime_service = RuntimeServiceId::new(widest("billing_runner-", 'r')).unwrap();
    let correlation_id = AuditCorrelationId::new(widest("Use.7f3a_trace:b/c@d+e~f[1]-Z9", 'c')).unwrap();
    let base = request(&tenant());

    for location in [
        InjectionLocation::header(widest("X-Api+Key.v1_~", 'h')).unwrap(),
        InjectionLocation::query_parameter(widest("api~key.v1_-", 'q')).unwrap(),
        InjectionLocation::body_field(widest("auth.client-secret.v1_", 'b')).unwrap(),
    ] {
        let request = ReleaseRequest::new(
            base.tenant().clone(),
            runtime_service.clone(),
            credential.clone(),
            base.destination().clone(),
            location.clone(),
            correlation_id.clone(),
        );

        let release = release_record(&release_of(request.clone()), stamp()).unwrap();
        let denial = denial_record(&DenialRecord::new(request, DenialReason::OwnerNotHeld), stamp()).unwrap();

        let Some(Detail::CredentialUse(usage)) = release.message().detail.as_ref() else {
            panic!("not a credential use: {:?}", release.message().detail);
        };
        assert_eq!(usage.location.as_deref(), Some(location.to_string().as_str()));
        for record in [&release, &denial] {
            assert_eq!(record.message().request_id.as_deref(), Some(correlation_id.as_str()));
        }
    }
}

#[test]
fn a_release_is_a_must_credential_use_of_the_runtime_service_before_the_value_leaves() {
    let tenant = tenant();
    let record = release_record(&release_of(request(&tenant)), stamp()).unwrap();
    let message = record.message();

    assert_eq!(record.obligation(), Obligation::BeforeEffect);
    assert_eq!(
        record.subject().render(),
        format!("platform.v1.audit.credential.{OWNER}.release")
    );
    assert_eq!(message.tenant.value, tenant.as_str());
    assert_eq!(message.project.value, OWNER);
    assert_eq!(message.actor.kind, Some(service_actor()));
    assert_eq!(message.request_id.as_deref(), Some(CORRELATION_ID));
    let Some(Detail::CredentialUse(usage)) = message.detail.as_ref() else {
        panic!("not a credential use: {:?}", message.detail);
    };
    let credential = credential_name(&identity().key());
    assert_eq!(
        usage.kind,
        EnumValue::from(CredentialUseKind::CREDENTIAL_USE_KIND_RELEASE)
    );
    assert_eq!(usage.credential, credential);
    assert_eq!(usage.binding_id, credential);
    assert_eq!(usage.destination.as_deref(), Some("api.github.com:8443"));
    assert_eq!(usage.location.as_deref(), Some("header:authorization"));
    assert_eq!(usage.decision_record_id, None);
}

#[test]
fn a_denial_is_a_decision_that_stands_with_its_reason() {
    let tenant = tenant();
    let denial = DenialRecord::new(request(&tenant), DenialReason::OwnerNotHeld);
    let record = denial_record(&denial, stamp()).unwrap();
    let message = record.message();

    assert_eq!(record.obligation(), Obligation::Stands);
    assert_eq!(
        record.subject().render(),
        format!("platform.v1.audit.decision.{OWNER}.deny")
    );
    assert_eq!(message.tenant.value, tenant.as_str());
    assert_eq!(message.actor.kind, Some(service_actor()));
    assert_eq!(message.request_id.as_deref(), Some(CORRELATION_ID));
    assert_eq!(
        message.attribution.decision,
        EnumValue::from(AuditDecision::AUDIT_DECISION_DENY)
    );
    assert_eq!(message.attribution.reason_code.as_deref(), Some("owner_not_held"));
    let Some(Detail::GatewayDecision(decision)) = message.detail.as_ref() else {
        panic!("not a decision: {:?}", message.detail);
    };
    assert_eq!(
        decision.surface,
        EnumValue::from(GatewaySurface::GATEWAY_SURFACE_SECRETS_RELEASE)
    );
    assert_eq!(decision.target, credential_name(&identity().key()));
}

#[test]
fn every_denial_reason_is_a_reason_code_a_record_carries() {
    let reasons = [
        DenialReason::RequesterXkeyInvalid,
        DenialReason::UnknownTenant,
        DenialReason::OwnerNotHeld,
        DenialReason::NoReleasePolicy,
        DenialReason::PolicyRefused(RuntimeReleaseDeniedError::RuntimeService {
            runtime_service: Some(RuntimeServiceId::new("intruder").unwrap()),
        }),
        DenialReason::PolicyRefused(RuntimeReleaseDeniedError::Host {
            host: Some("evil.example".to_owned()),
        }),
        DenialReason::PolicyRefused(RuntimeReleaseDeniedError::InjectionLocation {
            location: InjectionLocation::query_parameter("token").unwrap(),
        }),
        DenialReason::UnknownCredential,
        DenialReason::NotReleasable,
        DenialReason::VersionUnreadable,
        DenialReason::VerifierOnly,
    ];

    for reason in reasons {
        let code = reason.code();
        let record = denial_record(&DenialRecord::new(request(&tenant()), reason), stamp()).unwrap();
        assert_eq!(record.message().attribution.reason_code.as_deref(), Some(code));
    }
}
