use trogon_sandbox_contract::{LeaseId, TenantId};

use super::dns_label::validate_dns1123_label;
use super::dns_label_error::DnsLabelError;
use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn atespace(value: &str) -> Result<Atespace, Box<dyn std::error::Error>> {
    Ok(Atespace::for_tenant(&TenantId::new(value)?)?)
}

fn generated_lease_id() -> LeaseId {
    LeaseId::new(crate::ActorClaimId::generate().as_str())
}

#[test]
fn a_generated_lease_id_round_trips_through_the_actor_name() -> TestResult {
    for _ in 0..256 {
        let lease_id = generated_lease_id();
        let actor = ActorName::for_lease(&lease_id)?;
        assert_eq!(actor.lease_id().as_ref(), Some(&lease_id));
    }
    Ok(())
}

#[test]
fn every_generated_actor_name_is_a_valid_dns1123_label() -> TestResult {
    for _ in 0..256 {
        let actor = ActorName::for_lease(&generated_lease_id())?;
        assert_eq!(validate_dns1123_label(actor.as_str()), Ok(()));
        assert_eq!(actor.to_string(), actor.as_str());
        assert!(!actor.is_named_lease());
    }
    Ok(())
}

#[test]
fn a_named_actor_name_is_a_valid_label_for_arbitrary_lease_names() -> TestResult {
    let space = atespace("tenant-a")?;
    let long = "x".repeat(4096);
    for name in ["my sandbox", "UPPER", "with/slash", "emoji-🦀", "", long.as_str()] {
        let actor = ActorName::for_named_lease(&space, name);
        assert_eq!(validate_dns1123_label(actor.as_str()), Ok(()));
        assert!(actor.is_named_lease());
    }
    Ok(())
}

#[test]
fn a_named_actor_name_is_stable_across_lease_ids_and_scoped_by_tenant() -> TestResult {
    let a = atespace("tenant-a")?;
    let b = atespace("tenant-b")?;
    assert_eq!(
        ActorName::for_named_lease(&a, "build"),
        ActorName::for_named_lease(&a, "build")
    );
    assert_ne!(
        ActorName::for_named_lease(&a, "build"),
        ActorName::for_named_lease(&b, "build"),
        "two tenants using the same lease name must not share an actor"
    );
    Ok(())
}

#[test]
fn named_actor_names_cannot_be_confused_with_lease_derived_ones() -> TestResult {
    let actor = ActorName::for_named_lease(&atespace("tenant-a")?, "build");
    assert!(actor.lease_id().is_none());
    assert!(is_managed(actor.as_str()));
    Ok(())
}

#[test]
fn length_prefixing_stops_adjacent_name_fields_from_colliding() -> TestResult {
    assert_ne!(
        ActorName::for_named_lease(&atespace("t")?, "ab"),
        ActorName::for_named_lease(&atespace("ta")?, "b")
    );
    Ok(())
}

#[test]
fn a_tenant_that_is_not_already_a_label_is_rejected_rather_than_normalized() -> TestResult {
    for tenant in ["Tenant_A", "tenant a", "-leading", "trailing-", "UPPER"] {
        let error = Atespace::for_tenant(&TenantId::new(tenant)?)
            .err()
            .ok_or("must not be silently normalized")?;
        assert!(matches!(error, NamingError::TenantNotADnsLabel { .. }));
    }
    Ok(())
}

#[test]
fn a_tenant_over_the_label_limit_is_rejected() -> TestResult {
    let error = Atespace::for_tenant(&TenantId::new("t".repeat(64))?)
        .err()
        .ok_or("too long")?;
    assert!(matches!(error, NamingError::TenantNotADnsLabel { .. }));
    assert_eq!(
        validate_dns1123_label(&"t".repeat(64)),
        Err(DnsLabelError::TooLong { length: 64 })
    );
    Ok(())
}

#[test]
fn a_valid_tenant_is_used_verbatim() -> TestResult {
    let space = atespace("tenant-a1")?;
    assert_eq!(space.as_str(), "tenant-a1");
    assert_eq!(space.to_string(), "tenant-a1");
    Ok(())
}

#[test]
fn label_validation_names_the_first_problem() {
    assert_eq!(validate_dns1123_label(""), Err(DnsLabelError::Empty));
    assert_eq!(
        validate_dns1123_label("a_b"),
        Err(DnsLabelError::InvalidCharacter { character: '_' })
    );
    assert_eq!(validate_dns1123_label("-a"), Err(DnsLabelError::BadStart));
    assert_eq!(validate_dns1123_label("a-"), Err(DnsLabelError::BadEnd));
    assert_eq!(validate_dns1123_label("a-b"), Ok(()));
}

#[test]
fn a_lease_id_that_is_not_alphanumeric_is_rejected() -> TestResult {
    let error = ActorName::for_lease(&LeaseId::new("has/slash"))
        .err()
        .ok_or("rejected")?;
    assert!(matches!(error, NamingError::LeaseIdNotALabel { .. }));
    Ok(())
}

#[test]
fn an_empty_lease_id_is_rejected() -> TestResult {
    let error = ActorName::for_lease(&LeaseId::new("")).err().ok_or("rejected")?;
    assert!(matches!(error, NamingError::LeaseIdNotALabel { .. }));
    Ok(())
}

#[test]
fn a_lease_id_too_long_for_a_label_is_rejected() -> TestResult {
    let error = ActorName::for_lease(&LeaseId::new("a".repeat(64)))
        .err()
        .ok_or("rejected")?;
    assert!(matches!(error, NamingError::LeaseIdNotALabel { .. }));
    Ok(())
}

#[test]
fn unmanaged_names_are_left_alone() {
    assert!(!is_managed("golden"));
    assert!(!is_managed("operator-actor"));
    assert!(ActorName::from_existing("operator-actor").is_none());
    assert!(ActorName::from_existing("sbx-01j").is_some());
}
