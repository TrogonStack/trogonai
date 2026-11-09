use buffa::{EnumValue, MessageField};
use trogonai_proto::platform::audit::v1::{Attribution, WriterLifecycle};

use super::*;
use crate::change_facts::ChangeFacts;
use crate::change_terminal::ChangeTerminal;
use crate::credential_facts::CredentialFacts;
use crate::credential_terminal::CredentialTerminal;
use crate::fixtures::{allow, best_effort, change, credential, deny, name, scope, stamper};
use crate::record::Record;

fn release() -> Record {
    let facts = CredentialFacts::new(CredentialTerminal::Release, name("github_token"), name("binding_1"));
    Record::credential_use(scope(), facts, stamper().stamp())
}

fn credential_change() -> Record {
    let facts = ChangeFacts {
        kind: ChangeTerminal::Credential,
        resource_name: name("credentials/github"),
        event_type: name("credential_rotated"),
        source_stream: name("CREDENTIAL_EVENTS"),
        stream_sequence: 7,
        event_id: name("evt_2"),
    };
    Record::change(scope(), facts, stamper().stamp())
}

fn lifecycle(kind: WriterLifecycleKind) -> AuditRecord {
    AuditRecord {
        detail: Some(
            WriterLifecycle {
                kind: EnumValue::from(kind),
                reset_reason: None,
            }
            .into(),
        ),
        ..Default::default()
    }
}

fn records() -> Vec<Record> {
    let stamper = stamper();
    vec![
        allow(&stamper),
        deny(&stamper),
        best_effort(&stamper),
        credential(&stamper),
        release(),
        change(&stamper),
        credential_change(),
    ]
}

#[test]
fn every_record_agrees_with_its_own_subject() {
    for record in records() {
        let subject = record.subject().render();
        assert!(subject_agrees(&subject, record.message()), "{subject}");
    }
}

#[test]
fn a_record_disagrees_with_another_kind_of_subject() {
    let records = records();
    for record in &records {
        for other in &records {
            let subject = other.subject().render();
            if subject != record.subject().render() {
                assert!(!subject_agrees(&subject, record.message()), "{subject}");
            }
        }
    }
}

#[test]
fn a_lifecycle_record_agrees_with_any_component() {
    let start = lifecycle(WriterLifecycleKind::Start);
    let reset = lifecycle(WriterLifecycleKind::ChainReset);

    assert!(subject_agrees("platform.v1.audit.integrity.egress_proxy.start", &start));
    assert!(subject_agrees("platform.v1.audit.integrity.a2a_gateway.start", &start));
    assert!(subject_agrees(
        "platform.v1.audit.integrity.egress_proxy.chain_reset",
        &reset
    ));
    assert!(!subject_agrees(
        "platform.v1.audit.integrity.egress_proxy.start",
        &reset
    ));
}

#[test]
fn a_subject_must_be_an_audit_subject() {
    let record = deny(&stamper());
    let message = record.message();
    let subject = record.subject().render();
    assert!(subject_agrees(&subject, message));

    for wrong in [
        "trogon.v1.audit.decision.project_1.deny",
        "platform.v2.audit.decision.project_1.deny",
        "platform.v1.audits.decision.project_1.deny",
        "platform.v1.audit.decision.project_2.deny",
        "platform.v1.audit.decision.project_1",
        "platform.v1.audit.decision.project_1.deny.extra",
    ] {
        assert!(!subject_agrees(wrong, message), "{wrong}");
    }
}

#[test]
fn an_unspecified_detail_agrees_with_no_subject() {
    let stamper = stamper();
    let mut no_detail = deny(&stamper).message().clone();
    no_detail.detail = None;
    let mut no_attribution = deny(&stamper).message().clone();
    no_attribution.attribution = MessageField::none();
    let mut unspecified_decision = deny(&stamper).message().clone();
    unspecified_decision.attribution = MessageField::some(Attribution::default());
    let mut unspecified_credential = credential(&stamper).message().clone();
    if let Some(Detail::CredentialUse(credential)) = &mut unspecified_credential.detail {
        credential.kind = EnumValue::from(CredentialUseKind::CREDENTIAL_USE_KIND_UNSPECIFIED);
    }
    let mut unspecified_change = change(&stamper).message().clone();
    if let Some(Detail::Change(change)) = &mut unspecified_change.detail {
        change.kind = EnumValue::from(ChangeKind::CHANGE_KIND_UNSPECIFIED);
    }
    let unspecified_lifecycle = lifecycle(WriterLifecycleKind::Unspecified);

    let cases = [
        ("platform.v1.audit.decision.project_1.deny", no_detail),
        ("platform.v1.audit.decision.project_1.deny", no_attribution),
        ("platform.v1.audit.decision.project_1.deny", unspecified_decision),
        ("platform.v1.audit.credential.project_1.inject", unspecified_credential),
        ("platform.v1.audit.change.project_1.policy", unspecified_change),
        ("platform.v1.audit.integrity.egress_proxy.start", unspecified_lifecycle),
    ];
    for (subject, message) in cases {
        assert!(!subject_agrees(subject, &message), "{subject}");
    }
}

#[test]
fn an_unknown_detail_value_agrees_with_no_subject() {
    let stamper = stamper();
    let mut decision = deny(&stamper).message().clone();
    decision.attribution.get_or_insert_default().decision = EnumValue::Unknown(99);
    let mut credential_use = credential(&stamper).message().clone();
    if let Some(Detail::CredentialUse(credential)) = &mut credential_use.detail {
        credential.kind = EnumValue::Unknown(99);
    }
    let mut changed = change(&stamper).message().clone();
    if let Some(Detail::Change(change)) = &mut changed.detail {
        change.kind = EnumValue::Unknown(99);
    }
    let mut lifecycle_record = lifecycle(WriterLifecycleKind::Start);
    if let Some(Detail::WriterLifecycle(lifecycle)) = &mut lifecycle_record.detail {
        lifecycle.kind = EnumValue::Unknown(99);
    }

    let cases = [
        ("platform.v1.audit.decision.project_1.deny", decision),
        ("platform.v1.audit.credential.project_1.inject", credential_use),
        ("platform.v1.audit.change.project_1.policy", changed),
        ("platform.v1.audit.integrity.egress_proxy.start", lifecycle_record),
    ];
    for (subject, message) in cases {
        assert!(!subject_agrees(subject, &message), "{subject}");
    }
}

#[test]
fn a_readiness_probe_cannot_claim_a_reset_or_decision_attribution() {
    let mut probe = lifecycle(WriterLifecycleKind::ReadinessProbe);
    probe.class = EnumValue::from(trogonai_proto::platform::audit::v1::AuditClass::Must);
    let subject = "platform.v1.audit.integrity.egress_proxy.readiness_probe";
    assert!(subject_agrees(subject, &probe));
    let mut attributed = probe.clone();
    attributed.attribution = deny(&stamper()).message().attribution.clone();
    assert!(!subject_agrees(subject, &attributed));
    let mut best_effort = probe.clone();
    best_effort.class = EnumValue::from(trogonai_proto::platform::audit::v1::AuditClass::BestEffort);
    assert!(!subject_agrees(subject, &best_effort));
    let Some(Detail::WriterLifecycle(lifecycle)) = probe.detail.as_mut() else {
        panic!("readiness lifecycle");
    };
    lifecycle.reset_reason = Some(EnumValue::from(
        trogonai_proto::platform::audit::v1::ChainResetReason::ProcessStartWithoutResume,
    ));
    assert!(!subject_agrees(subject, &probe));
}
