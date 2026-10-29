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
