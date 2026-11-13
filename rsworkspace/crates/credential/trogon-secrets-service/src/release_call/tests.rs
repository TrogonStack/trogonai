use buffa::MessageField;
use nkeys::{KeyPair, XKey};
use trogon_credential_domain::{InjectionLocation, ReleaseHostError};
use trogonai_proto::secrets::exchange::v1alpha1::ReleaseLocation;
use trogonai_proto::secrets::exchange::v1alpha1::release_location::Value;

use super::*;
use crate::TenantAccount;
use crate::fixtures::{HOST, release_subject, release_wire, request, tenant};

fn caller(tenant: &TenantAccount) -> ReleaseCaller {
    ReleaseCaller::from_subject(&release_subject(tenant)).unwrap()
}

fn read(edit: impl FnOnce(&mut ResolveForReleaseRequest)) -> Result<ReleaseCall, ReleaseCallError> {
    let tenant = tenant();
    let mut wire = release_wire(&XKey::new().public_key());
    edit(&mut wire);
    ReleaseCall::from_wire(&caller(&tenant), wire)
}

#[test]
fn a_call_missing_a_field_is_invalid() {
    assert_eq!(
        read(|wire| wire.credential = MessageField::none()).err(),
        Some(ReleaseCallError::MissingCredential)
    );
    assert_eq!(
        read(|wire| wire.destination_host = MessageField::none()).err(),
        Some(ReleaseCallError::MissingDestination)
    );
    assert_eq!(
        read(|wire| wire.location = MessageField::none()).err(),
        Some(ReleaseCallError::MissingLocation)
    );
    assert_eq!(
        read(|wire| wire.correlation_id = MessageField::none()).err(),
        Some(ReleaseCallError::MissingCorrelationId)
    );
    assert_eq!(
        read(|wire| wire.requester_xkey = MessageField::none()).err(),
        Some(ReleaseCallError::MissingRequesterXkey)
    );
}

#[test]
fn a_location_that_names_no_place_is_invalid() {
    assert_eq!(
        read(|wire| wire.location = MessageField::some(ReleaseLocation::default())).err(),
        Some(ReleaseCallError::MissingLocation)
    );
}

#[test]
fn a_credential_address_the_domain_rejects_is_invalid() {
    assert!(matches!(
        read(|wire| wire.credential.get_or_insert_default().credential_id = "fake-not-a-uuid".to_owned()),
        Err(ReleaseCallError::CredentialId(_))
    ));
    assert!(matches!(
        read(|wire| wire.credential.get_or_insert_default().credential_id = String::new()),
        Err(ReleaseCallError::CredentialId(_))
    ));
    assert!(matches!(
        read(|wire| wire.credential.get_or_insert_default().owner_id = "..".to_owned()),
        Err(ReleaseCallError::OwnerId(_))
    ));
    assert!(matches!(
        read(|wire| wire.credential.get_or_insert_default().owner_id = String::new()),
        Err(ReleaseCallError::OwnerId(_))
    ));
}

#[test]
fn a_destination_host_that_is_a_pattern_is_invalid() {
    for host in [
        "*.github.com",
        "api.github.com/v3",
        "api github.com",
        " api.github.com",
        "api.github.com ",
    ] {
        assert_eq!(
            read(|wire| wire.destination_host.get_or_insert_default().host = host.to_owned()).err(),
            Some(ReleaseCallError::DestinationNotExact),
            "{host:?}"
        );
    }
}

#[test]
fn a_destination_host_the_domain_rejects_is_invalid() {
    assert_eq!(
        read(|wire| wire.destination_host.get_or_insert_default().host = String::new()).err(),
        Some(ReleaseCallError::DestinationHost(ReleaseHostError::Empty))
    );
    assert_eq!(
        read(|wire| wire.destination_host.get_or_insert_default().host = format!("{HOST}:443")).err(),
        Some(ReleaseCallError::DestinationHost(ReleaseHostError::PortNotAllowed))
    );
}

#[test]
fn a_destination_port_outside_one_to_65535_is_invalid() {
    for port in [0, 65_536, u32::MAX] {
        assert_eq!(
            read(|wire| wire.destination_host.get_or_insert_default().port = port).err(),
            Some(ReleaseCallError::DestinationPort(port)),
            "{port}"
        );
    }
}

#[test]
fn a_location_the_domain_rejects_is_invalid() {
    for value in [
        Value::Header("x-api key".to_owned()),
        Value::Header(String::new()),
        Value::QueryParameter("api+key".to_owned()),
        Value::BodyField("auth..token".to_owned()),
    ] {
        assert!(
            matches!(
                read(|wire| wire.location.get_or_insert_default().value = Some(value.clone())),
                Err(ReleaseCallError::Location(_))
            ),
            "{value:?}"
        );
    }
}

#[test]
fn a_correlation_id_the_domain_rejects_is_invalid() {
    for value in ["", "use 1"] {
        assert!(
            matches!(
                read(|wire| wire.correlation_id.get_or_insert_default().value = value.to_owned()),
                Err(ReleaseCallError::CorrelationId(_))
            ),
            "{value:?}"
        );
    }
}

#[test]
fn a_requester_xkey_off_the_curve_key_pattern_is_invalid() {
    let xkey = XKey::new().public_key();
    for key in [
        String::new(),
        KeyPair::new_account().public_key(),
        xkey.to_ascii_lowercase(),
        format!("{xkey}A"),
        xkey[..55].to_owned(),
        format!("{}1", &xkey[..55]),
    ] {
        assert_eq!(
            read(|wire| wire.requester_xkey.get_or_insert_default().public_key = key.clone()).err(),
            Some(ReleaseCallError::RequesterXkeyPattern),
            "{key:?}"
        );
    }
}

#[test]
fn a_requester_xkey_on_the_pattern_is_left_for_the_release_to_judge() {
    let xkey = XKey::new().public_key();
    let last = if xkey.ends_with('A') { 'B' } else { 'A' };
    let corrupted = format!("{}{last}", &xkey[..55]);
    let call = read(|wire| wire.requester_xkey.get_or_insert_default().public_key = corrupted.clone()).unwrap();
    assert_eq!(call.requester_xkey(), corrupted);
}

#[test]
fn a_call_takes_its_caller_from_the_subject_and_the_rest_from_the_payload() {
    let tenant = tenant();
    let xkey = XKey::new().public_key();
    let call = ReleaseCall::from_wire(&caller(&tenant), release_wire(&xkey)).unwrap();
    assert_eq!(call.request(), &request(&tenant));
    assert_eq!(call.requester_xkey(), xkey);
}

#[test]
fn a_call_reads_each_kind_of_location() {
    for (value, location) in [
        (
            Value::Header("X-Api-Key".to_owned()),
            InjectionLocation::header("x-api-key").unwrap(),
        ),
        (
            Value::QueryParameter("api_key".to_owned()),
            InjectionLocation::query_parameter("api_key").unwrap(),
        ),
        (
            Value::BodyField("auth.token".to_owned()),
            InjectionLocation::body_field("auth.token").unwrap(),
        ),
    ] {
        let call = read(|wire| wire.location.get_or_insert_default().value = Some(value)).unwrap();
        assert_eq!(call.request().location(), &location);
    }
}
