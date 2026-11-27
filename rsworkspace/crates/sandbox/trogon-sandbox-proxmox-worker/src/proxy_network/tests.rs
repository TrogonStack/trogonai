use super::*;

fn network() -> Result<ProxyNetwork, Box<dyn std::error::Error>> {
    Ok(ProxyNetwork {
        bridge: "vmbr9".into(),
        addresses: vec!["10.9.0.2/24".parse()?, "10.9.0.3/24".parse()?],
        gateway: "10.9.0.1".parse()?,
        bind_address: "10.9.0.1".parse()?,
        advertised_address: "10.9.0.1".parse()?,
        antispoof: AntiSpoof::Require,
    })
}

#[test]
fn a_consistent_network_validates() -> Result<(), Box<dyn std::error::Error>> {
    network()?.validate(2)?;
    Ok(())
}

#[test]
fn a_pool_smaller_than_the_capacity_is_refused() -> Result<(), Box<dyn std::error::Error>> {
    assert!(matches!(network()?.validate(3), Err(ProxyNetworkError::PoolTooSmall)));
    Ok(())
}

#[test]
fn unusable_addresses_and_bridges_are_refused() -> Result<(), Box<dyn std::error::Error>> {
    let mut gateway_address = network()?;
    gateway_address.addresses[0] = "10.9.0.1/24".parse()?;
    assert!(gateway_address.validate(1).is_err());
    let mut broadcast = network()?;
    broadcast.addresses[0] = "10.9.0.255/24".parse()?;
    assert!(broadcast.validate(1).is_err());
    let mut duplicate = network()?;
    duplicate.addresses[1] = duplicate.addresses[0];
    assert!(matches!(duplicate.validate(2), Err(ProxyNetworkError::Duplicate(_))));
    let mut bridge = network()?;
    bridge.bridge = "vmbr 9".into();
    assert!(matches!(bridge.validate(1), Err(ProxyNetworkError::Bridge)));
    let mut loopback = network()?;
    loopback.advertised_address = Ipv4Addr::LOCALHOST;
    assert!(matches!(loopback.validate(1), Err(ProxyNetworkError::Advertised)));
    Ok(())
}
