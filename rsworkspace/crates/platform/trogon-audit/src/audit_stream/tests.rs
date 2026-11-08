use async_nats::jetstream::stream::{DiscardPolicy, RetentionPolicy, StorageType};

use super::*;
use crate::audit_subject::AuditSubject;
use crate::change_terminal::ChangeTerminal;
use crate::checkpoint_terminal::CheckpointTerminal;
use crate::credential_terminal::CredentialTerminal;
use crate::decision_terminal::DecisionTerminal;
use crate::fixtures::token;
use crate::integrity_terminal::IntegrityTerminal;

fn one() -> NonZeroUsize {
    NonZeroUsize::MIN
}

fn three() -> NonZeroUsize {
    NonZeroUsize::new(3).unwrap()
}

#[test]
fn names_the_two_streams() {
    assert_eq!(AuditStream::Audit.name(), "AUDIT");
    assert_eq!(AuditStream::Checkpoint.name(), "AUDIT_CHECKPOINT");
    assert_eq!(AuditStream::ALL, [AuditStream::Audit, AuditStream::Checkpoint]);
}

#[test]
fn audit_takes_four_kinds_and_checkpoint_takes_one() {
    assert_eq!(
        AuditStream::Audit.subjects(),
        [
            "platform.v1.audit.decision.>",
            "platform.v1.audit.credential.>",
            "platform.v1.audit.change.>",
            "platform.v1.audit.integrity.>",
        ]
    );
    assert_eq!(AuditStream::Checkpoint.subjects(), ["platform.v1.audit.checkpoint.>"]);
}

/// The `>` filter of a family matches every subject beneath it, which is how
/// JetStream reads the stream's subjects.
fn captures(stream: AuditStream, subject: &str) -> bool {
    stream.subjects().iter().any(|filter| {
        let family = filter.strip_suffix('>').unwrap();
        subject.starts_with(family) && subject.len() > family.len()
    })
}

#[test]
fn every_subject_a_record_can_take_lands_in_exactly_the_stream_its_kind_belongs_to() {
    let project = || token("project_1");
    let component = || token("egress_proxy");
    let in_audit = [
        AuditSubject::decision(project(), DecisionTerminal::Allow),
        AuditSubject::decision(project(), DecisionTerminal::Deny),
        AuditSubject::decision(project(), DecisionTerminal::WouldDeny),
        AuditSubject::credential(project(), CredentialTerminal::Release),
        AuditSubject::credential(project(), CredentialTerminal::Inject),
        AuditSubject::change(project(), ChangeTerminal::Policy),
        AuditSubject::change(project(), ChangeTerminal::Credential),
        AuditSubject::integrity(component(), IntegrityTerminal::Start),
        AuditSubject::integrity(component(), IntegrityTerminal::ChainReset),
    ];
    for subject in in_audit {
        let subject = subject.render();
        assert!(captures(AuditStream::Audit, &subject), "{subject}");
        assert!(!captures(AuditStream::Checkpoint, &subject), "{subject}");
    }
    let checkpoint = AuditSubject::checkpoint(component(), CheckpointTerminal::Signed).render();
    assert!(captures(AuditStream::Checkpoint, &checkpoint));
    assert!(!captures(AuditStream::Audit, &checkpoint));
}

#[test]
fn config_holds_every_guarantee_the_adr_names() {
    for (stream, replicas) in [(AuditStream::Audit, one()), (AuditStream::Checkpoint, three())] {
        let config = stream.config(replicas);
        assert_eq!(config.name, stream.name());
        assert_eq!(config.subjects, stream.subjects());
        assert_eq!(config.storage, StorageType::File);
        assert_eq!(config.retention, RetentionPolicy::Limits);
        assert_eq!(config.discard, DiscardPolicy::New);
        assert!(config.deny_delete);
        assert!(config.deny_purge);
        assert_eq!(config.duplicate_window, Duration::from_secs(120));
        assert!(config.max_age.is_zero());
        assert_eq!(config.num_replicas, replicas.get());
    }
}

#[test]
fn the_configuration_it_creates_is_one_it_accepts() {
    for stream in AuditStream::ALL {
        assert_eq!(stream.validate(three(), &stream.config(three())), Ok(()));
    }
}

#[test]
fn refuses_each_way_a_stream_could_weaken_the_guarantees() {
    type Weaken = fn(&mut Config);
    let cases: [(Weaken, AuditStreamMismatchError); 12] = [
        (
            |c| c.storage = StorageType::Memory,
            AuditStreamMismatchError::Storage { stream: "AUDIT" },
        ),
        (
            |c| c.retention = RetentionPolicy::Interest,
            AuditStreamMismatchError::Retention { stream: "AUDIT" },
        ),
        (
            |c| c.discard = DiscardPolicy::Old,
            AuditStreamMismatchError::Discard { stream: "AUDIT" },
        ),
        (
            |c| c.deny_delete = false,
            AuditStreamMismatchError::DenyDelete { stream: "AUDIT" },
        ),
        (
            |c| c.deny_purge = false,
            AuditStreamMismatchError::DenyPurge { stream: "AUDIT" },
        ),
        (|c| c.no_ack = true, AuditStreamMismatchError::NoAck { stream: "AUDIT" }),
        (
            |c| c.max_age = Duration::from_secs(3600),
            AuditStreamMismatchError::MaxAge { stream: "AUDIT" },
        ),
        (
            |c| c.allow_message_ttl = true,
            AuditStreamMismatchError::MessageTtl { stream: "AUDIT" },
        ),
        (
            |c| c.allow_rollup = true,
            AuditStreamMismatchError::Rollup { stream: "AUDIT" },
        ),
        (
            |c| c.max_messages_per_subject = 1,
            AuditStreamMismatchError::MaxMessagesPerSubject { stream: "AUDIT" },
        ),
        (
            |c| c.duplicate_window = Duration::from_secs(30),
            AuditStreamMismatchError::DuplicateWindow { stream: "AUDIT" },
        ),
        (
            |c| c.num_replicas = 1,
            AuditStreamMismatchError::Replicas { stream: "AUDIT" },
        ),
    ];
    for (weaken, expected) in cases {
        let mut actual = AuditStream::Audit.config(three());
        weaken(&mut actual);
        assert_eq!(AuditStream::Audit.validate(three(), &actual), Err(expected));
    }
}

#[test]
fn refuses_a_stream_missing_a_subject_and_names_it() {
    let mut actual = AuditStream::Audit.config(one());
    actual.subjects.retain(|subject| !subject.contains("credential"));
    assert_eq!(
        AuditStream::Audit.validate(one(), &actual),
        Err(AuditStreamMismatchError::MissingSubject {
            stream: "AUDIT",
            subject: "platform.v1.audit.credential.>".to_owned(),
        })
    );
}

#[test]
fn names_its_own_stream_in_the_error() {
    let mut actual = AuditStream::Checkpoint.config(one());
    actual.discard = DiscardPolicy::Old;
    assert_eq!(
        AuditStream::Checkpoint.validate(one(), &actual),
        Err(AuditStreamMismatchError::Discard {
            stream: "AUDIT_CHECKPOINT"
        })
    );
}

#[test]
fn leaves_capacity_and_stronger_settings_alone() {
    let mut actual = AuditStream::Audit.config(one());
    actual.max_bytes = 1 << 30;
    actual.max_messages = 1_000_000;
    actual.max_message_size = 1 << 20;
    actual.max_messages_per_subject = -1;
    actual.num_replicas = 5;
    actual.duplicate_window = Duration::from_secs(600);
    actual.subjects.push("extra.subject".to_owned());
    assert_eq!(AuditStream::Audit.validate(one(), &actual), Ok(()));
}
