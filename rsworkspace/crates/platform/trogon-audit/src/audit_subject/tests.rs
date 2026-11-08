use trogon_nats::validate_published_subject;

use super::*;
use crate::fixtures::token;

#[test]
fn renders_every_kind_and_terminal() {
    let project = || token("project_1");
    let component = || token("egress_proxy");
    let cases = [
        (
            AuditSubject::decision(project(), DecisionTerminal::Allow),
            "platform.v1.audit.decision.project_1.allow",
        ),
        (
            AuditSubject::decision(project(), DecisionTerminal::Deny),
            "platform.v1.audit.decision.project_1.deny",
        ),
        (
            AuditSubject::decision(project(), DecisionTerminal::WouldDeny),
            "platform.v1.audit.decision.project_1.would_deny",
        ),
        (
            AuditSubject::credential(project(), CredentialTerminal::Release),
            "platform.v1.audit.credential.project_1.release",
        ),
        (
            AuditSubject::credential(project(), CredentialTerminal::Inject),
            "platform.v1.audit.credential.project_1.inject",
        ),
        (
            AuditSubject::change(project(), ChangeTerminal::Policy),
            "platform.v1.audit.change.project_1.policy",
        ),
        (
            AuditSubject::change(project(), ChangeTerminal::Credential),
            "platform.v1.audit.change.project_1.credential",
        ),
        (
            AuditSubject::integrity(component(), IntegrityTerminal::Start),
            "platform.v1.audit.integrity.egress_proxy.start",
        ),
        (
            AuditSubject::integrity(component(), IntegrityTerminal::ChainReset),
            "platform.v1.audit.integrity.egress_proxy.chain_reset",
        ),
        (
            AuditSubject::checkpoint(component(), CheckpointTerminal::Signed),
            "platform.v1.audit.checkpoint.egress_proxy.signed",
        ),
    ];
    for (subject, expected) in cases {
        assert_eq!(subject.render(), expected);
    }
}

#[test]
fn carries_no_tenant_token() {
    let subject = AuditSubject::decision(token("project_1"), DecisionTerminal::Allow).render();
    assert!(!subject.contains("tenant"));
    assert!(subject.starts_with("platform.v1.audit."));
}

#[test]
fn conforms_to_the_published_subject_rules_at_the_longest_token() {
    let longest = token(&"p".repeat(128));
    let subjects = [
        AuditSubject::decision(longest.clone(), DecisionTerminal::WouldDeny),
        AuditSubject::credential(longest.clone(), CredentialTerminal::Release),
        AuditSubject::change(longest.clone(), ChangeTerminal::Credential),
        AuditSubject::integrity(longest.clone(), IntegrityTerminal::ChainReset),
        AuditSubject::checkpoint(longest, CheckpointTerminal::Signed),
    ];
    for subject in subjects {
        validate_published_subject(&subject.render()).unwrap();
    }
}

#[test]
fn a_token_cannot_smuggle_in_a_wildcard_or_a_separator() {
    for bad in ["a.b", "*", ">", "a b"] {
        assert!(trogon_nats::NatsToken::new(bad).is_err(), "{bad}");
    }
}
