//! Host-side anti-spoofing for the guest's raw L2 path.
//!
//! The guest owns its whole network stack, so it can forge the source MAC, the source IP
//! or an 802.1Q tag. Three layers close that, all inside the lease's network namespace:
//!
//! - a `netdev` ingress chain on the TAP that drops VLAN-tagged frames and any frame whose
//!   source MAC is not the one the VMM assigned to the guest, counting each drop;
//! - `ip saddr` pinned to the guest address in the namespace's accept rules;
//! - strict reverse-path filtering (`rp_filter=1`) on the TAP.
//!
//! Residual risk: a guest can still send frames with the right MAC and IP, which is
//! exactly the traffic the proxy policy governs. ARP is not filtered beyond the MAC check.

use std::net::Ipv4Addr;

/// The `netdev` ruleset for `device`, accepting only frames sourced from `guest_mac`.
///
/// The leading `table`/`delete table` pair makes loading idempotent, so a TAP that was
/// recreated (and lost its hook) can be re-guarded with the same text.
pub(crate) fn l2_ruleset(device: &str, guest_mac: &str) -> String {
    format!(
        r#"table netdev {TABLE}
delete table netdev {TABLE}
table netdev {TABLE} {{
 counter {SPOOF_COUNTER} {{ }}
 chain ingress {{ type filter hook ingress device "{device}" priority -500; policy accept;
 ether type {{ 8021q, 8021ad }} counter name "{SPOOF_COUNTER}" drop;
 ether saddr != {guest_mac} counter name "{SPOOF_COUNTER}" drop;
 }}
}}
"#
    )
}

/// The `ip saddr` clause that pins forwarded or input traffic to the guest address.
pub(crate) fn saddr_match(guest: Ipv4Addr) -> String {
    format!("ip saddr {guest}")
}

/// The sysctl that turns on strict reverse-path filtering for `device`.
pub(crate) fn rp_filter_sysctl(device: &str) -> String {
    format!("net.ipv4.conf.{device}.rp_filter=1")
}

/// Reads the drop counter out of `nft -j list counter` output.
pub(crate) fn parse_drop_count(json: &str) -> anyhow::Result<u64> {
    let value: serde_json::Value = serde_json::from_str(json)?;
    value["nftables"]
        .as_array()
        .into_iter()
        .flatten()
        .find_map(|item| item["counter"]["packets"].as_u64())
        .ok_or_else(|| anyhow::anyhow!("no counter in nft output"))
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod root_tests;

mod constants;
pub(crate) use constants::{SPOOF_COUNTER, TABLE};
