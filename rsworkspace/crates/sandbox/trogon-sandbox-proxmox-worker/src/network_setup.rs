use std::net::Ipv4Addr;

use serde_json::Value;

use crate::api_bodies::NicBody;
use crate::{AntiSpoof, NetworkSetupError, ProxmoxApi, ProxyNetwork, VmId};

fn flag_on(value: &Value) -> bool {
    value.as_u64() == Some(1) || value.as_bool() == Some(true) || value.as_str().is_some_and(|text| text == "1")
}

/// What is wrong with the guest NIC's anti-spoofing, given what Proxmox reports back.
///
/// `options` is the VM's firewall options, `ip_filter` the entries of its `ipfilter-net0` set and `net0` the NIC
/// definition from the VM config.
pub(crate) fn antispoof_findings(options: &Value, ip_filter: &[Value], net0: &str, address: Ipv4Addr) -> Vec<String> {
    let mut findings = Vec::new();
    for key in ["enable", "macfilter", "ipfilter"] {
        if !flag_on(&options[key]) {
            findings.push(format!("VM firewall option `{key}` is not enabled"));
        }
    }
    if !net0.split(',').any(|part| part == "firewall=1") {
        findings.push("net0 does not have `firewall=1`, so the VM firewall is bypassed".to_owned());
    }
    let expected = format!("{address}/32");
    let exact = matches!(ip_filter, [entry] if entry["cidr"].as_str() == Some(expected.as_str()));
    if !exact {
        findings.push(format!("`ipfilter-net0` is not exactly {expected}"));
    }
    findings
}

/// Replaces everything the clone inherited with a firewall that lets the guest reach only its proxy, then reads the
/// result back and checks the NIC cannot spoof its MAC or IP.
pub(crate) async fn configure(
    api: &ProxmoxApi,
    vm: VmId,
    network: &ProxyNetwork,
    slot: usize,
    port: u16,
) -> Result<(), NetworkSetupError> {
    api.require_firewall_running().await?;
    api.clear_firewall_rules(vm).await?;
    api.lock_down_firewall(vm).await?;
    let address = network.addresses.get(slot).ok_or(NetworkSetupError::MissingSlot)?;
    api.set_ip_filter(vm, address.addr()).await?;
    api.allow_outbound(vm, network.advertised_address, port).await?;
    api.update_config(
        vm,
        &NicBody {
            net0: format!("virtio,bridge={},firewall=1", network.bridge),
            ipconfig0: format!("ip={address},gw={}", network.gateway),
        },
    )
    .await?;
    verify_antispoof(api, vm, network, address.addr()).await
}

async fn verify_antispoof(
    api: &ProxmoxApi,
    vm: VmId,
    network: &ProxyNetwork,
    address: Ipv4Addr,
) -> Result<(), NetworkSetupError> {
    let options = api.firewall_options(vm).await?;
    let entries = api.ip_filter_entries(vm).await?;
    let config = api.config(vm).await?;
    let findings = antispoof_findings(&options, &entries, config["net0"].as_str().unwrap_or_default(), address);
    if findings.is_empty() {
        return Ok(());
    }
    let findings = findings.join("; ");
    match network.antispoof {
        AntiSpoof::Require => Err(NetworkSetupError::AntiSpoof(findings)),
        AntiSpoof::Warn => {
            tracing::warn!(%vm, %findings, "guest NIC anti-spoofing could not be verified");
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests;
