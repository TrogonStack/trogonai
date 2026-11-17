use trogon_credential_domain::InjectionLocation;
use trogonai_proto::secrets::exchange::v1alpha1::release_location::Value;

use crate::fixtures::{
    CORRELATION_ID, CREDENTIAL_ID, HEADER, HOST, OWNER, PORT, credential, destination, request, request_with, tenant,
};

#[test]
fn carries_no_identity_beyond_the_credential_address() {
    let wire = request(&tenant()).to_wire("fake-requester-xkey".to_owned());

    let address = wire.credential.into_option().unwrap();
    assert_eq!(address.credential_id, CREDENTIAL_ID);
    assert_eq!(address.owner_id, OWNER);
}

#[test]
fn names_the_destination_correlation_and_requester_xkey() {
    let wire = request(&tenant()).to_wire("fake-requester-xkey".to_owned());

    let host = wire.destination_host.into_option().unwrap();
    assert_eq!((host.host.as_str(), host.port), (HOST, u32::from(PORT)));
    assert_eq!(wire.correlation_id.into_option().unwrap().value, CORRELATION_ID);
    assert_eq!(
        wire.requester_xkey.into_option().unwrap().public_key,
        "fake-requester-xkey"
    );
}

#[test]
fn names_each_location_by_its_own_placement() {
    let cases = [
        (
            InjectionLocation::header(HEADER).unwrap(),
            Value::Header(HEADER.to_owned()),
        ),
        (
            InjectionLocation::query_parameter("access_token").unwrap(),
            Value::QueryParameter("access_token".to_owned()),
        ),
        (
            InjectionLocation::body_field("auth.token").unwrap(),
            Value::BodyField("auth.token".to_owned()),
        ),
    ];
    for (location, expected) in cases {
        let wire = request_with(tenant(), location).to_wire("fake-requester-xkey".to_owned());

        assert_eq!(wire.location.into_option().unwrap().value, Some(expected));
    }
}

#[test]
fn keeps_what_it_was_built_from() {
    let tenant = tenant();

    let request = request(&tenant);

    assert_eq!(request.tenant(), &tenant);
    assert_eq!(request.credential(), &credential());
    assert_eq!(request.destination(), &destination());
    assert_eq!(request.location(), &InjectionLocation::header(HEADER).unwrap());
    assert_eq!(request.correlation_id().as_str(), CORRELATION_ID);
}
