use std::net::Ipv4Addr;

use reqwest::Method;
use serde_json::Value;

use crate::api_bodies::{FirewallOptionsBody, FirewallRuleBody, IpSetBody, IpSetEntryBody};
use crate::constants::IPFILTER_SET;
use crate::{ProxmoxApi, ProxmoxApiError, VmId};

impl ProxmoxApi {
    /// Refuses to continue unless the cluster firewall is on and its service is running.
    pub async fn require_firewall_running(&self) -> Result<(), ProxmoxApiError> {
        let cluster: Value = self.request(Method::GET, &["cluster", "firewall", "options"]).await?;
        if cluster["enable"].as_u64() != Some(1) {
            return Err(ProxmoxApiError::ClusterFirewallDisabled);
        }
        let service: Value = self
            .request(Method::GET, &self.node_path(&["services", "pve-firewall", "state"]))
            .await?;
        if service["state"] != "running" {
            return Err(ProxmoxApiError::FirewallStopped);
        }
        Ok(())
    }

    /// Deletes every firewall rule the VM inherited from its template.
    pub async fn clear_firewall_rules(&self, vm: VmId) -> Result<(), ProxmoxApiError> {
        let vm = vm.to_string();
        let rules: Vec<Value> = self
            .request(Method::GET, &self.vm_path(&vm, &["firewall", "rules"]))
            .await?;
        let mut positions = rules
            .iter()
            .map(|rule| rule["pos"].as_u64().ok_or(ProxmoxApiError::RuleWithoutPosition))
            .collect::<Result<Vec<_>, _>>()?;
        positions.sort_unstable_by(|first, second| second.cmp(first));
        for position in positions {
            let position = position.to_string();
            let _: Value = self
                .request(Method::DELETE, &self.vm_path(&vm, &["firewall", "rules", &position]))
                .await?;
        }
        Ok(())
    }

    pub(crate) async fn lock_down_firewall(&self, vm: VmId) -> Result<(), ProxmoxApiError> {
        let vm = vm.to_string();
        let _: Value = self
            .request_with(
                Method::PUT,
                &self.vm_path(&vm, &["firewall", "options"]),
                &FirewallOptionsBody::locked_down(),
            )
            .await?;
        Ok(())
    }

    /// The VM's firewall options as Proxmox reports them.
    pub async fn firewall_options(&self, vm: VmId) -> Result<Value, ProxmoxApiError> {
        let vm = vm.to_string();
        self.request(Method::GET, &self.vm_path(&vm, &["firewall", "options"]))
            .await
    }

    /// The entries of the VM's `ipfilter-net0` set.
    pub async fn ip_filter_entries(&self, vm: VmId) -> Result<Vec<Value>, ProxmoxApiError> {
        let vm = vm.to_string();
        self.request(Method::GET, &self.vm_path(&vm, &["firewall", "ipset", IPFILTER_SET]))
            .await
    }

    /// Lets the guest reach exactly one TCP address and port, ahead of every other rule.
    pub(crate) async fn allow_outbound(&self, vm: VmId, address: Ipv4Addr, port: u16) -> Result<(), ProxmoxApiError> {
        let vm = vm.to_string();
        let rule = FirewallRuleBody {
            direction: "out",
            action: "ACCEPT",
            proto: "tcp",
            dest: address.to_string(),
            dport: port.to_string(),
            enable: 1,
            pos: 0,
        };
        let _: Value = self
            .request_with(Method::POST, &self.vm_path(&vm, &["firewall", "rules"]), &rule)
            .await?;
        Ok(())
    }

    /// Pins the guest NIC to a single IPv4 address.
    pub async fn set_ip_filter(&self, vm: VmId, address: Ipv4Addr) -> Result<(), ProxmoxApiError> {
        let vm = vm.to_string();
        let sets: Vec<Value> = self
            .request(Method::GET, &self.vm_path(&vm, &["firewall", "ipset"]))
            .await?;
        if sets.iter().any(|set| set["name"] == IPFILTER_SET) {
            let _: Value = self
                .request(Method::DELETE, &self.vm_path(&vm, &["firewall", "ipset", IPFILTER_SET]))
                .await?;
        }
        let _: Value = self
            .request_with(
                Method::POST,
                &self.vm_path(&vm, &["firewall", "ipset"]),
                &IpSetBody { name: IPFILTER_SET },
            )
            .await?;
        let _: Value = self
            .request_with(
                Method::POST,
                &self.vm_path(&vm, &["firewall", "ipset", IPFILTER_SET]),
                &IpSetEntryBody {
                    cidr: format!("{address}/32"),
                },
            )
            .await?;
        Ok(())
    }
}
