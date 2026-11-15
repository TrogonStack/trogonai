use super::*;

#[test]
fn accepts_subject_safe_ids() {
    assert_eq!(
        RuntimeServiceId::new("trogon-gateway").unwrap().as_str(),
        "trogon-gateway"
    );
    assert_eq!(RuntimeServiceId::new("worker_2").unwrap().as_str(), "worker_2");
}

#[test]
fn rejects_subject_unsafe_ids() {
    assert_eq!(RuntimeServiceId::new(""), Err(RuntimeServiceIdError::Empty));
    assert_eq!(
        RuntimeServiceId::new("trogon.gateway"),
        Err(RuntimeServiceIdError::InvalidCharacter('.'))
    );
    assert_eq!(
        RuntimeServiceId::new("trogon/gateway"),
        Err(RuntimeServiceIdError::InvalidCharacter('/'))
    );
    assert_eq!(
        RuntimeServiceId::new("trogon gateway"),
        Err(RuntimeServiceIdError::InvalidCharacter(' '))
    );
    assert_eq!(
        RuntimeServiceId::new("*"),
        Err(RuntimeServiceIdError::InvalidCharacter('*'))
    );
}

#[test]
fn unrestricted_permits_an_absent_identity() {
    assert!(AllowedRuntimeServices::Unrestricted.permits(None));
}

#[test]
fn restricted_denies_an_absent_identity() {
    let services = AllowedRuntimeServices::only(["trogon-gateway"]).unwrap();
    let allowed = RuntimeServiceId::new("trogon-gateway").unwrap();
    let other = RuntimeServiceId::new("some-other-worker").unwrap();

    assert!(services.permits(Some(&allowed)));
    assert!(!services.permits(Some(&other)));
    assert!(!services.permits(None));
}

#[test]
fn empty_allow_list_denies_everything() {
    let services = AllowedRuntimeServices::Only(BTreeSet::new());
    let any = RuntimeServiceId::new("trogon-gateway").unwrap();

    assert!(!services.permits(Some(&any)));
    assert!(!services.permits(None));
}

#[test]
fn rejects_ids_over_the_length_limit_counted_in_characters() {
    let over_limit = "a".repeat(MAX_RUNTIME_SERVICE_ID_LEN + 1);

    assert!(RuntimeServiceId::new("a".repeat(MAX_RUNTIME_SERVICE_ID_LEN)).is_ok());
    assert_eq!(
        RuntimeServiceId::new(over_limit),
        Err(RuntimeServiceIdError::TooLong(MAX_RUNTIME_SERVICE_ID_LEN + 1))
    );
}

#[test]
fn renders_the_id_in_debug_and_display() {
    let id = RuntimeServiceId::new("trogon-gateway").unwrap();

    assert_eq!(id.to_string(), "trogon-gateway");
    assert_eq!(format!("{id:?}"), "RuntimeServiceId(\"trogon-gateway\")");
}

#[test]
fn only_rejects_an_invalid_member_and_reports_restriction_state() {
    assert_eq!(
        AllowedRuntimeServices::only(["trogon-gateway", "bad.id"]),
        Err(RuntimeServiceIdError::InvalidCharacter('.'))
    );
    assert!(AllowedRuntimeServices::Unrestricted.is_unrestricted());
    assert!(
        !AllowedRuntimeServices::only(["trogon-gateway"])
            .unwrap()
            .is_unrestricted()
    );
}
