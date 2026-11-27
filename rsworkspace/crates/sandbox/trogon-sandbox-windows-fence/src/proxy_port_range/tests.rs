use super::*;

#[test]
fn a_bind_failure_other_than_a_port_in_use_is_the_result() -> Result<(), Box<dyn std::error::Error>> {
    let range = ProxyPortRange::new(60100, 60105)?;
    let mut tried = Vec::new();
    let result = range.listen_with(&[], |port| {
        tried.push(port);
        Err(io::Error::from(io::ErrorKind::PermissionDenied))
    });
    let Err(error) = result else {
        return Err("expected an error".into());
    };
    assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    assert_eq!(tried, [60100]);
    Ok(())
}
