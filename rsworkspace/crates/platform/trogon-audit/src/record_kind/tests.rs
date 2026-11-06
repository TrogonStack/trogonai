use super::*;

fn decision(outcome: DecisionOutcome, read_only: ReadOnly, break_glass: bool) -> RecordKind {
    RecordKind::Decision {
        outcome,
        read_only,
        break_glass,
    }
}

const ENFORCED_ALLOW: DecisionOutcome = DecisionOutcome::Allow(Enforcement::Enforce);
const DRY_RUN_ALLOW: DecisionOutcome = DecisionOutcome::Allow(Enforcement::DryRun);

#[test]
fn every_enforced_deny_is_must_whatever_the_declaration() {
    for read_only in [ReadOnly::Declared, ReadOnly::Undeclared] {
        for break_glass in [false, true] {
            let kind = decision(DecisionOutcome::Deny, read_only, break_glass);
            assert_eq!(kind.audit_class(), AuditClass::Must);
            assert_eq!(kind.obligation(), Obligation::Stands);
        }
    }
}

#[test]
fn an_enforced_allow_is_must_unless_the_surface_declared_the_method_read_only() {
    let undeclared = decision(ENFORCED_ALLOW, ReadOnly::Undeclared, false);
    assert_eq!(undeclared.audit_class(), AuditClass::Must);
    assert_eq!(undeclared.obligation(), Obligation::BeforeEffect);

    let declared = decision(ENFORCED_ALLOW, ReadOnly::Declared, false);
    assert_eq!(declared.audit_class(), AuditClass::BestEffort);
    assert_eq!(declared.obligation(), Obligation::BestEffort);
}

#[test]
fn a_break_glass_allow_stays_must_even_on_a_read_only_method() {
    let kind = decision(ENFORCED_ALLOW, ReadOnly::Declared, true);
    assert_eq!(kind.audit_class(), AuditClass::Must);
    assert_eq!(kind.obligation(), Obligation::BeforeEffect);
}

#[test]
fn every_dry_run_record_is_best_effort() {
    for outcome in [DRY_RUN_ALLOW, DecisionOutcome::WouldDeny] {
        for read_only in [ReadOnly::Declared, ReadOnly::Undeclared] {
            for break_glass in [false, true] {
                let kind = decision(outcome, read_only, break_glass);
                assert_eq!(kind.audit_class(), AuditClass::BestEffort);
                assert_eq!(kind.obligation(), Obligation::BestEffort);
            }
        }
    }
}

#[test]
fn credential_change_and_integrity_records_are_must_before_the_effect() {
    for kind in [RecordKind::CredentialUse, RecordKind::Change, RecordKind::Integrity] {
        assert_eq!(kind.audit_class(), AuditClass::Must);
        assert_eq!(kind.obligation(), Obligation::BeforeEffect);
    }
}
