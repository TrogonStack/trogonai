use super::*;

#[test]
fn ingress_chain_drops_vlan_tags_and_foreign_source_macs_with_a_counter() {
    let rules = l2_ruleset("tap0", "06:00:ac:10:00:02");
    assert!(rules.contains(r#"hook ingress device "tap0" priority -500"#));
    assert!(rules.contains(r#"ether type { 8021q, 8021ad } counter name "spoofed" drop"#));
    assert!(rules.contains(r#"ether saddr != 06:00:ac:10:00:02 counter name "spoofed" drop"#));
    assert!(rules.contains("counter spoofed { }"));
    assert!(rules.starts_with("table netdev sandbox_fc_l2\ndelete table netdev sandbox_fc_l2\n"));
}

#[test]
fn helpers_render_the_pinned_address_and_the_sysctl() {
    assert_eq!(saddr_match(Ipv4Addr::new(172, 16, 0, 2)), "ip saddr 172.16.0.2");
    assert_eq!(rp_filter_sysctl("tap0"), "net.ipv4.conf.tap0.rp_filter=1");
}

#[test]
fn the_drop_count_is_read_from_nft_json() -> Result<(), Box<dyn std::error::Error>> {
    let json = r#"{"nftables":[{"metainfo":{"version":"1.0.9"}},{"counter":{"family":"netdev","name":"spoofed","table":"sandbox_fc_l2","handle":2,"packets":7,"bytes":420}}]}"#;
    assert_eq!(parse_drop_count(json)?, 7);
    assert!(parse_drop_count(r#"{"nftables":[]}"#).is_err());
    Ok(())
}
