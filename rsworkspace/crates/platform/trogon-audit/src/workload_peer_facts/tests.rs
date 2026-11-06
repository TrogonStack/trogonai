use buffa::{EnumValue, MessageField};
use buffa_types::google::protobuf::Timestamp;
use trogonai_proto::platform::audit::v1::audit_record::Detail;
use trogonai_proto::platform::workload_identity::v1alpha1::{RegistrationBinding, WorkloadPeerEvidence, WorkloadRole};

use crate::fixtures::{decision_facts, scope, stamper};
use crate::{DecisionOutcome, Enforcement, Obligation, ReadOnly, Record, WorkloadPeerEvidenceFacts, WorkloadPeerFactsError};

fn peer() -> WorkloadPeerEvidence {
    WorkloadPeerEvidence {
        peer_binding: MessageField::some(RegistrationBinding {
            spiffe_id: "spiffe://example.test/runtime/one".into(),
            tenant: "tenant_1".into(),
            project: "project_1".into(),
            role: EnumValue::from(WorkloadRole::Agent),
            application_id: Some("agent-7".into()),
        }),
        peer_leaf_sha256: vec![1; 32],
        peer_path_expires_at: MessageField::some(Timestamp { seconds: 1_799_999_999, nanos: 0, ..Default::default() }),
        recovery_fence: 1,
        registration_revision: 2,
        trust_revision: 3,
        verifier_source_revision: 4,
        verifier_activation_revision: 5,
        verifier_bundle_sha256: vec![6; 32],
    }
}

fn evidence(record: &Record) -> &WorkloadPeerEvidence {
    let Some(Detail::GatewayDecision(decision)) = record.message().detail.as_ref() else { panic!("gateway detail"); };
    decision.workload_peer.as_option().expect("authenticated peer evidence")
}

#[test]
fn static_legacy_decisions_cannot_claim_authenticated_workload_evidence() {
    let record = Record::decision(scope(), decision_facts(DecisionOutcome::Deny, ReadOnly::Undeclared), stamper().stamp());
    let Some(Detail::GatewayDecision(decision)) = record.message().detail.as_ref() else { panic!("gateway detail"); };
    assert!(decision.workload_peer.is_unset());
}

#[test]
fn authenticated_records_distinguish_runtimes_that_share_one_application_identity() {
    let first = peer();
    let mut second = peer();
    second.peer_binding.as_option_mut().unwrap().spiffe_id = "spiffe://example.test/runtime/two".into();
    second.peer_leaf_sha256 = vec![2; 32];
    let facts = || decision_facts(DecisionOutcome::Allow(Enforcement::Enforce), ReadOnly::Undeclared);
    let first_record = Record::authenticated_workload_decision(scope(), facts(), WorkloadPeerEvidenceFacts::try_from(first.clone()).unwrap(), stamper().stamp()).unwrap();
    let second_record = Record::authenticated_workload_decision(scope(), facts(), WorkloadPeerEvidenceFacts::try_from(second.clone()).unwrap(), stamper().stamp()).unwrap();
    assert_eq!(first_record.message().actor, second_record.message().actor);
    assert_eq!(evidence(&first_record), &first);
    assert_eq!(evidence(&second_record), &second);
    assert_ne!(evidence(&first_record), evidence(&second_record));
}

#[test]
fn an_expiry_refusal_retains_the_previously_authenticated_peer() {
    let message = peer();
    let record = Record::authenticated_workload_decision(scope(), decision_facts(DecisionOutcome::Deny, ReadOnly::Undeclared), WorkloadPeerEvidenceFacts::try_from(message.clone()).unwrap(), stamper().stamp()).unwrap();
    assert_eq!(record.obligation(), Obligation::Stands);
    assert_eq!(evidence(&record), &message);
}

#[test]
fn workload_evidence_refuses_malformed_authenticated_metadata() {
    let mut message = peer();
    message.peer_leaf_sha256.pop();
    assert_eq!(WorkloadPeerEvidenceFacts::try_from(message), Err(WorkloadPeerFactsError::Fingerprint));
}
