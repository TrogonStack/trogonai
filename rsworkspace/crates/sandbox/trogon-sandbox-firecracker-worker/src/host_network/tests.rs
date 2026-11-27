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
