//! Builds Kubernetes `NetworkPolicy` objects (and, for domain-based rules, the sidecar egress-proxy machinery) from
//! a [`trogon_sandbox_contract::NetworkPolicy`].
//!
//! This module is a pure translation layer: given a policy plus the pod labels, namespace and job name it applies
//! to, it returns the Kubernetes objects needed to enforce that policy. It does not talk to the API itself; callers
//! create and delete the returned objects.
//!
//! # Deny-all
//!
//! A `DenyAll` policy produces a real `NetworkPolicySpec` with `policyTypes: ["Egress"]`. An empty `egress: []`
//! rule list under a `NetworkPolicy` selecting the pod means the CNI plugin allows zero egress destinations,
//! including DNS. There is no DNS carve-out, matching the documented "DenyAll denies all outbound traffic,
//! including DNS" semantics of the contract policy.
//!
//! # Domain-based rules
//!
//! The core Kubernetes `NetworkPolicy` API has no primitive for matching on hostnames or SNI, only pod selectors,
//! namespace selectors, and IP blocks. So when a `UserDefined` policy has non-empty domain `rules`, those rules
//! cannot be expressed as `NetworkPolicy` egress rules. Instead, [`build_egress_sidecar`] returns a sidecar
//! container running the egress proxy plus an init container that installs an iptables `REDIRECT` rule, so that
//! HTTP(S)-shaped traffic gets transparently routed through the proxy, which enforces the domain rules itself via
//! SNI and Host-header inspection. This is a real enforcement path, but it is not equivalent to a CNI plugin with
//! native DNS-aware FQDN enforcement: it only covers traffic that is HTTP(S)-shaped and actually goes through the
//! iptables redirect, and it depends on the proxy correctly parsing SNI and Host headers rather than the CNI
//! blocking non-matching destinations at the packet level.

mod constants;

use std::collections::BTreeMap;

use k8s_openapi::api::core::v1::{
    Capabilities, ConfigMap, ConfigMapVolumeSource, Container, EnvVar, SecurityContext, Volume, VolumeMount,
};
use k8s_openapi::api::networking::v1::{
    IPBlock, NetworkPolicy, NetworkPolicyEgressRule, NetworkPolicyPeer, NetworkPolicySpec,
};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{LabelSelector, ObjectMeta};
use trogon_sandbox_contract::{DomainRuleAction, IpRange, NetworkPolicy as ContractNetworkPolicy, ProxyUid};

use crate::{
    EgressSidecarResources, EgressViolationEvents, K8sNetworkPolicyError, egress_redirect_command, exempt_uid_env_pair,
};

pub(crate) use constants::EGRESS_PROXY_CONFIGMAP_KEY;
use constants::{
    EGRESS_PROXY_CA_FILE, EGRESS_PROXY_LISTEN_PORT, EGRESS_PROXY_POLICY_MOUNT_PATH, EGRESS_PROXY_POLICY_VOLUME_NAME,
};
pub use constants::{EGRESS_PROXY_CA_MOUNT_PATH, EGRESS_PROXY_CA_VOLUME_NAME, EGRESS_PROXY_IMAGE};

/// Deterministic name for the `NetworkPolicy` object derived from the job name, so relaunches against the same job
/// name reuse (or collide predictably with, rather than orphan) the same object.
pub fn network_policy_name(job_name: &str) -> String {
    format!("{job_name}-netpol")
}

/// Deterministic name for the ConfigMap backing the egress-proxy sidecar's policy file.
pub fn egress_proxy_configmap_name(job_name: &str) -> String {
    format!("{job_name}-egress-policy")
}

/// Builds the `NetworkPolicy` object (if any) that enforces `policy` for the pods matching `sandbox_pod_labels` in
/// `namespace`.
///
/// - `AllowAll` gives `None`: no `NetworkPolicy` object at all. Kubernetes default-allows egress for pods that no
///   `NetworkPolicy` selects, which matches "no filtering".
/// - `DenyAll` gives `Some` with an empty egress rule list, which denies all egress (including DNS) at the CNI
///   level.
/// - `UserDefined` gives `Some`, always (even when everything is empty), because it is deny-by-default: one
///   `NetworkPolicyEgressRule` is emitted per `allow_ranges` entry, each carved out by any `deny_ranges` entries
///   that are a genuine CIDR subset of that specific allow range. Domain-based `rules` do not change what this
///   function returns; see [`build_egress_sidecar`] for how those are enforced.
pub fn build_network_policy_manifest(
    sandbox_pod_labels: &BTreeMap<String, String>,
    policy: &ContractNetworkPolicy,
    namespace: &str,
    job_name: &str,
) -> Option<NetworkPolicy> {
    let egress = match policy {
        ContractNetworkPolicy::AllowAll => return None,
        ContractNetworkPolicy::DenyAll => vec![],
        ContractNetworkPolicy::UserDefined {
            allow_ranges,
            deny_ranges,
            ..
        } => build_egress_rules(allow_ranges, deny_ranges),
    };

    Some(NetworkPolicy {
        metadata: ObjectMeta {
            name: Some(network_policy_name(job_name)),
            namespace: Some(namespace.to_string()),
            ..Default::default()
        },
        spec: Some(NetworkPolicySpec {
            pod_selector: Some(LabelSelector {
                match_labels: Some(sandbox_pod_labels.clone()),
                ..Default::default()
            }),
            policy_types: Some(vec!["Egress".to_string()]),
            egress: Some(egress),
            ..Default::default()
        }),
    })
}

/// Builds one `NetworkPolicyEgressRule` per `allow_ranges` entry, excluding any `deny_ranges` entries that are a
/// genuine CIDR subset of that specific allow range. If `allow_ranges` is empty, returns an empty rule list (deny
/// all, since `UserDefined` is deny-by-default).
fn build_egress_rules(allow_ranges: &[IpRange], deny_ranges: &[IpRange]) -> Vec<NetworkPolicyEgressRule> {
    allow_ranges
        .iter()
        .map(|allow| {
            let except = except_list_for(allow, deny_ranges);
            NetworkPolicyEgressRule {
                to: Some(vec![NetworkPolicyPeer {
                    ip_block: Some(IPBlock {
                        cidr: allow.as_str().to_string(),
                        except: (!except.is_empty()).then_some(except),
                    }),
                    ..Default::default()
                }]),
                ports: None,
            }
        })
        .collect()
}

/// Returns the subset of `deny_ranges` (as raw CIDR strings) that are genuinely contained within `allow`. A deny
/// range that is not a real subset of the allow range (disjoint, overlapping-but-not-contained, or a supernet of
/// the allow range) is skipped rather than causing an error: this function builds a manifest, not a validator.
fn except_list_for(allow: &IpRange, deny_ranges: &[IpRange]) -> Vec<String> {
    let allow_net = ip_net(allow);

    deny_ranges
        .iter()
        .filter(|deny| {
            allow_net
                .zip(ip_net(deny))
                .is_some_and(|(allow_net, deny_net)| allow_net.contains(&deny_net))
        })
        .map(|deny| deny.as_str().to_string())
        .collect()
}

fn ip_net(range: &IpRange) -> Option<ipnet::IpNet> {
    ipnet::IpNet::new(range.address(), range.prefix_len()).ok()
}

/// Whether `policy` has a `Transform` or `Forward` rule, so the sidecar must terminate TLS and publish its CA.
/// A policy with only pass-through rules never triggers CA-trust wiring: the CA file would never get populated, and
/// pointing a client at a permanently-empty CA bundle breaks every TLS connection, not just the ones this policy is
/// meant to intercept.
fn requires_tls_intercept(policy: &ContractNetworkPolicy) -> bool {
    match policy {
        ContractNetworkPolicy::AllowAll | ContractNetworkPolicy::DenyAll => false,
        ContractNetworkPolicy::UserDefined { rules, .. } => rules.iter().any(|rule| {
            matches!(
                rule.action,
                DomainRuleAction::Transform(_) | DomainRuleAction::Forward(_)
            )
        }),
    }
}

/// Builds the sidecar proxy, the iptables-redirect init container and the ConfigMap needed to enforce
/// domain-based (`rules`) egress rules, since core Kubernetes `NetworkPolicy` has no FQDN or SNI matching
/// primitive. Only call this when `policy` is `UserDefined` with non-empty `rules`; callers should skip this
/// machinery entirely when `rules` is empty (a plain `NetworkPolicy` object suffices).
///
/// Since containers in the same Pod natively share a network namespace, both the sidecar and the init container
/// are simply added as siblings in the Pod's `containers` and `init_containers` lists. For the same reason the
/// `--uid-owner` exemption applies to every container in the Pod, so no other container may run as
/// [`ProxyUid::EGRESS_PROXY`].
pub fn build_egress_sidecar(
    policy: &ContractNetworkPolicy,
    job_name: &str,
    violation_events: Option<&EgressViolationEvents>,
) -> Result<EgressSidecarResources, K8sNetworkPolicyError> {
    let policy_json = serde_json::to_string(policy).map_err(K8sNetworkPolicyError::SerializePolicy)?;

    let configmap_name = egress_proxy_configmap_name(job_name);
    let policy_file_path = format!("{EGRESS_PROXY_POLICY_MOUNT_PATH}/{EGRESS_PROXY_CONFIGMAP_KEY}");

    let config_map = ConfigMap {
        metadata: ObjectMeta {
            name: Some(configmap_name.clone()),
            ..Default::default()
        },
        data: Some(BTreeMap::from([(EGRESS_PROXY_CONFIGMAP_KEY.to_string(), policy_json)])),
        ..Default::default()
    };

    let volume = Volume {
        name: EGRESS_PROXY_POLICY_VOLUME_NAME.to_string(),
        config_map: Some(ConfigMapVolumeSource {
            name: configmap_name,
            ..Default::default()
        }),
        ..Default::default()
    };

    let (exempt_uid_key, exempt_uid) = exempt_uid_env_pair();
    let mut sidecar_env = vec![
        env_var(exempt_uid_key, exempt_uid),
        env_var(
            "EGRESS_PROXY_LISTEN_ADDR",
            format!("0.0.0.0:{EGRESS_PROXY_LISTEN_PORT}"),
        ),
        env_var("EGRESS_PROXY_POLICY_FILE", policy_file_path),
    ];
    if let Some(events) = violation_events {
        sidecar_env.extend(
            events
                .env_pairs()
                .into_iter()
                .map(|(name, value)| env_var(name, value.to_string())),
        );
    }
    let mut sidecar_volume_mounts = vec![VolumeMount {
        name: EGRESS_PROXY_POLICY_VOLUME_NAME.to_string(),
        mount_path: EGRESS_PROXY_POLICY_MOUNT_PATH.to_string(),
        read_only: Some(true),
        ..Default::default()
    }];

    let ca_volume = requires_tls_intercept(policy).then(|| {
        sidecar_env.push(env_var(
            "EGRESS_PROXY_CA_OUT_FILE",
            format!("{EGRESS_PROXY_CA_MOUNT_PATH}/{EGRESS_PROXY_CA_FILE}"),
        ));
        sidecar_volume_mounts.push(VolumeMount {
            name: EGRESS_PROXY_CA_VOLUME_NAME.to_string(),
            mount_path: EGRESS_PROXY_CA_MOUNT_PATH.to_string(),
            ..Default::default()
        });
        Volume {
            name: EGRESS_PROXY_CA_VOLUME_NAME.to_string(),
            empty_dir: Some(Default::default()),
            ..Default::default()
        }
    });

    let sidecar_container = Container {
        name: "egress-proxy".to_string(),
        image: Some(EGRESS_PROXY_IMAGE.to_string()),
        env: Some(sidecar_env),
        volume_mounts: Some(sidecar_volume_mounts),
        security_context: Some(SecurityContext {
            run_as_user: Some(i64::from(ProxyUid::EGRESS_PROXY.get())),
            run_as_non_root: Some(true),
            allow_privilege_escalation: Some(false),
            read_only_root_filesystem: Some(true),
            capabilities: Some(Capabilities {
                drop: Some(vec!["ALL".to_string()]),
                ..Default::default()
            }),
            ..Default::default()
        }),
        ..Default::default()
    };

    let init_container = Container {
        name: "egress-proxy-iptables-init".to_string(),
        image: Some(EGRESS_PROXY_IMAGE.to_string()),
        command: Some(vec!["/bin/sh".to_string(), "-c".to_string()]),
        args: Some(vec![egress_redirect_command(EGRESS_PROXY_LISTEN_PORT)]),
        security_context: Some(SecurityContext {
            run_as_user: Some(0),
            run_as_non_root: Some(false),
            allow_privilege_escalation: Some(false),
            read_only_root_filesystem: Some(true),
            capabilities: Some(Capabilities {
                add: Some(vec!["NET_ADMIN".to_string()]),
                ..Default::default()
            }),
            ..Default::default()
        }),
        ..Default::default()
    };

    Ok(EgressSidecarResources {
        sidecar_container,
        init_container,
        config_map,
        volume,
        ca_volume,
    })
}

fn env_var(name: &str, value: String) -> EnvVar {
    EnvVar {
        name: name.to_string(),
        value: Some(value),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests;
