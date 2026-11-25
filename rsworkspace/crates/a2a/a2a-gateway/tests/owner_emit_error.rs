#![cfg(feature = "spicedb")]

use std::error::Error;

use a2a_gateway::policy::OwnerEmitError;

#[test]
fn owner_emission_failure_preserves_the_transport_source() -> std::io::Result<()> {
    let error = OwnerEmitError::from(tonic::Status::unavailable("backend unavailable"));
    let source = error
        .source()
        .and_then(|source| source.downcast_ref::<tonic::Status>())
        .ok_or_else(|| std::io::Error::other("owner emission failure must preserve its typed transport source"))?;
    assert_eq!(source.code(), tonic::Code::Unavailable);
    assert_eq!(source.message(), "backend unavailable");
    Ok(())
}
