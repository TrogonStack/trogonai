use super::*;

const EVERY_PORT: ProxyPortRange = ProxyPortRange::reported(1, u16::MAX);

#[test]
fn a_listener_is_bound_outside_the_range() -> Result<(), Box<dyn std::error::Error>> {
    let range = ProxyPortRange::new(60100, 60105)?;
    let listener = ProbeListener::bind_outside(range)?;
    assert!(!range.contains(listener.addr.port()));
    Ok(())
}

#[test]
fn no_candidate_is_outside_a_range_of_every_port() {
    assert!(candidate(EVERY_PORT).is_none());
}

#[test]
fn a_range_of_every_port_leaves_nowhere_to_bind() -> Result<(), Box<dyn std::error::Error>> {
    let Err(error) = ProbeListener::bind_outside(EVERY_PORT) else {
        return Err("expected an error".into());
    };
    assert_eq!(error.code(), Code::WfpVerifyBindFailed);
    assert!(error.message().contains("[1,65535] in 5 attempts"));
    Ok(())
}
