use serde_json::{Value, json};

use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn policy(change: impl FnOnce(&mut Value)) -> Result<NetworkPolicy, serde_json::Error> {
    // The vendor firewall cannot refuse opaque tunnels, so every policy it serves has to opt in.
    let mut wire =
        json!({"user_defined": {"rules": [], "allow_ranges": [], "deny_ranges": [], "opaque_tunnels": "allow"}});
    change(&mut wire["user_defined"]);
    serde_json::from_value(wire)
}

fn is_invalid(policy: &NetworkPolicy) -> bool {
    matches!(
        translate_network_policy(Some(policy)),
        Err(LeaseError::InvalidNetworkPolicy { .. })
    )
}

#[test]
fn unset_and_allow_all_leave_the_default() -> TestResult {
    assert_eq!(translate_network_policy(None)?, DaytonaNetwork::default());
    assert_eq!(
        translate_network_policy(Some(&NetworkPolicy::AllowAll))?,
        DaytonaNetwork::default()
    );
    Ok(())
}

#[test]
fn deny_all_and_a_policy_that_allows_nothing_block_everything() -> TestResult {
    assert_eq!(translate_network_policy(Some(&NetworkPolicy::DenyAll))?, blocked());
    assert_eq!(translate_network_policy(Some(&policy(|_| {})?))?, blocked());
    Ok(())
}

#[test]
fn ranges_and_ip_hosts_become_one_sorted_allow_list() -> TestResult {
    let policy = policy(|wire| {
        wire["rules"] = json!([{"domain": "10.0.0.1"}, {"domain": "[::1]"}, {"domain": "10.0.0.1"}]);
        wire["allow_ranges"] = json!(["192.168.0.0/16", "10.0.0.0/8"]);
    })?;
    assert_eq!(
        translate_network_policy(Some(&policy))?,
        DaytonaNetwork {
            block_all: None,
            allow_list: Some("10.0.0.0/8,10.0.0.1/32,192.168.0.0/16,::1/128".to_owned()),
        }
    );
    Ok(())
}

#[test]
fn a_single_address_allow_range_becomes_a_one_address_block() -> TestResult {
    let policy = policy(|wire| wire["allow_ranges"] = json!(["10.0.0.6", "fc00::1"]))?;
    assert_eq!(
        translate_network_policy(Some(&policy))?,
        DaytonaNetwork {
            block_all: None,
            allow_list: Some("10.0.0.6/32,fc00::1/128".to_owned()),
        }
    );
    Ok(())
}

#[test]
fn what_daytona_cannot_express_is_refused() -> TestResult {
    let cases = [
        json!({"rules": [{"domain": "10.0.0.1:8443"}]}),
        json!({"rules": [{"domain": "example.com"}]}),
        json!({"rules": [{"domain": "*.example.com"}]}),
        json!({"opaque_tunnels": "refuse"}),
        json!({"denied_domains": ["x.com"]}),
        json!({"request_rules": [{"action": "deny"}]}),
        json!({"deny_ranges": ["10.0.0.0/8"]}),
        json!({"allow_ranges": ["10.0.0.5:8080"]}),
        json!({"rules": [{
            "domain": "10.0.0.1",
            "action": {"forward": {"forward_url": "https://forward.example.com/"}}
        }]}),
    ];
    for case in cases {
        let policy = policy(|wire| {
            if let (Some(wire), Some(case)) = (wire.as_object_mut(), case.as_object()) {
                wire.extend(case.clone());
            }
        })?;
        assert!(is_invalid(&policy), "{policy:?}");
    }
    Ok(())
}

#[test]
fn more_than_ten_blocks_are_refused() -> TestResult {
    let ranges: Vec<String> = (0..=MAX_ALLOW_ENTRIES).map(|n| format!("10.{n}.0.0/16")).collect();
    let too_many = policy(|wire| wire["allow_ranges"] = json!(ranges))?;
    assert!(is_invalid(&too_many));
    let exact: Vec<String> = ranges.into_iter().take(MAX_ALLOW_ENTRIES).collect();
    let fits = policy(|wire| wire["allow_ranges"] = json!(exact))?;
    assert!(translate_network_policy(Some(&fits)).is_ok());
    Ok(())
}

#[test]
fn admission_refuses_every_restricting_policy_with_the_unverified_reason() -> TestResult {
    for restricted in [NetworkPolicy::DenyAll, policy(|_| {})?] {
        let refused = require_unrestricted_egress(Some(&restricted)).err().ok_or("admitted")?;
        assert!(matches!(
            refused,
            LeaseError::InvalidNetworkPolicy { ref reason } if reason == RESTRICTED_EGRESS_UNVERIFIED
        ));
    }
    assert_eq!(
        RESTRICTED_EGRESS_UNVERIFIED,
        "restricted Daytona egress requires verified provider enforcement, which is unavailable"
    );
    Ok(())
}

#[test]
fn admission_lets_unrestricted_egress_through() -> TestResult {
    require_unrestricted_egress(None)?;
    require_unrestricted_egress(Some(&NetworkPolicy::AllowAll))?;
    Ok(())
}

#[test]
fn a_policy_that_does_not_opt_in_to_opaque_tunnels_is_refused() -> TestResult {
    let silent = policy(|wire| {
        if let Some(wire) = wire.as_object_mut() {
            wire.remove("opaque_tunnels");
        }
    })?;
    assert!(is_invalid(&silent), "{silent:?}");
    Ok(())
}
