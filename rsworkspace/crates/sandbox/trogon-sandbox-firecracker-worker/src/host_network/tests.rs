use super::*;

#[test]
fn firewall_only_forwards_the_exact_proxy_endpoint_and_returns() -> Result<(), Box<dyn std::error::Error>> {
    let identity = Identity {
        token: "aabbccddeeff".into(),
        subnet: "10.231.0.0/30".parse()?,
        namespace_inode: None,
    };
    let rules = firewall(&identity, 12345);
    assert_eq!(rules.matches("policy drop").count(), 3);
    assert!(rules.contains(
        "iifname \"tap0\" oifname \"fcpaabbccddeeff\" ip saddr 172.16.0.2 ip daddr 10.231.0.1 tcp dport 12345 accept"
    ));
    assert!(rules.contains("tcp sport 12345 ct state established accept"));
    assert!(rules.contains("snat to 10.231.0.2"));
    assert!(!rules.contains("related"));
    Ok(())
}

#[test]
fn pools_and_recovery_identities_are_bounded() -> Result<(), Box<dyn std::error::Error>> {
    assert!(
        NetworkConfig {
            host_links: "10.231.0.0/24".parse()?
        }
        .validate(8)
        .is_ok()
    );
    assert!(
        NetworkConfig {
            host_links: "8.8.8.0/24".parse()?
        }
        .validate(8)
        .is_err()
    );
    assert!(
        NetworkConfig {
            host_links: "10.231.0.0/30".parse()?
        }
        .validate(8)
        .is_err()
    );
    assert!(
        Identity {
            token: "../../foreign".into(),
            subnet: "10.0.0.0/30".parse()?,
            namespace_inode: None
        }
        .validate()
        .is_err()
    );
    Ok(())
}

#[test]
fn a_host_route_that_cannot_be_read_refuses_the_subnet_choice() -> Result<(), Box<dyn std::error::Error>> {
    let subnet: Ipv4Net = "10.231.0.0/30".parse()?;
    let clear = vec![
        serde_json::json!({"dst": "default"}),
        serde_json::json!({"dst": "192.168.0.0/24"}),
        serde_json::json!({"dst": "192.168.1.7"}),
    ];
    assert!(!overlaps_existing_route(&clear, subnet)?);
    let colliding = vec![serde_json::json!({"dst": "10.231.0.0/16"})];
    assert!(overlaps_existing_route(&colliding, subnet)?);
    assert!(overlaps_existing_route(
        &[serde_json::json!({"dst": "10.231.0.2"})],
        subnet
    )?);
    assert!(overlaps_existing_route(&[serde_json::json!({"dst": "bogus"})], subnet).is_err());
    assert!(overlaps_existing_route(&[serde_json::json!({"gateway": "10.0.0.1"})], subnet).is_err());
    Ok(())
}
