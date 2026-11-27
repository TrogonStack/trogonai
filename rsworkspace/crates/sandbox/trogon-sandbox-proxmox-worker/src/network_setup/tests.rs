use serde_json::json;

use super::*;

const NET0: &str = "virtio=BC:24:11:00:00:01,bridge=vmbr9,firewall=1";

fn address() -> Result<Ipv4Addr, std::net::AddrParseError> {
    "10.9.0.2".parse()
}

#[test]
fn a_fully_filtered_nic_has_no_findings() -> Result<(), Box<dyn std::error::Error>> {
    let options = json!({"enable": 1, "macfilter": 1, "ipfilter": 1, "policy_out": "DROP"});
    let set = [json!({"cidr": "10.9.0.2/32"})];
    assert!(antispoof_findings(&options, &set, NET0, address()?).is_empty());
    Ok(())
}

#[test]
fn each_missing_filter_is_reported() -> Result<(), Box<dyn std::error::Error>> {
    let findings = antispoof_findings(
        &json!({"enable": 1, "macfilter": 0}),
        &[json!({"cidr": "10.9.0.0/24"})],
        "virtio,bridge=vmbr9",
        address()?,
    );
    assert_eq!(findings.len(), 4);
    assert!(findings.iter().any(|finding| finding.contains("macfilter")));
    assert!(findings.iter().any(|finding| finding.contains("`ipfilter`")));
    assert!(findings.iter().any(|finding| finding.contains("firewall=1")));
    assert!(findings.iter().any(|finding| finding.contains("10.9.0.2/32")));
    Ok(())
}
