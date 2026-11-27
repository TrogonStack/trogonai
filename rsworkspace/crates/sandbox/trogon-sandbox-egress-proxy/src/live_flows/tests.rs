use std::io;
use std::time::Duration;

use tokio::time::timeout;
use trogon_sandbox_contract::{DomainRule, DomainRuleAction};

use super::*;
use crate::resolve::{NameLookup, SsrfFloor};

/// One shared edge address answers for every name in the table.
struct Table(Mutex<HashMap<String, Vec<IpAddr>>>);

#[async_trait::async_trait]
impl NameLookup for Table {
    async fn lookup(&self, name: &str, _port: Port) -> io::Result<Vec<IpAddr>> {
        self.0
            .lock()
            .map_err(|_| io::Error::other("poisoned"))?
            .get(name)
            .cloned()
            .ok_or_else(|| io::ErrorKind::NotFound.into())
    }
}

const EDGE: &str = "203.0.113.7";

fn resolver(names: &[(&str, &str)]) -> Result<VettingResolver, Box<dyn std::error::Error>> {
    let table = names
        .iter()
        .map(|(name, ip)| Ok((name.to_string(), vec![ip.parse()?])))
        .collect::<Result<_, Box<dyn std::error::Error>>>()?;
    Ok(VettingResolver::new(
        Arc::new(Table(Mutex::new(table))),
        SsrfFloor::system(),
    ))
}

fn policy(allowed: &[&str]) -> Result<NetworkPolicy, Box<dyn std::error::Error>> {
    Ok(NetworkPolicy::UserDefined {
        rules: allowed
            .iter()
            .map(|domain| {
                Ok(DomainRule {
                    domain: domain.parse()?,
                    action: DomainRuleAction::PassThrough,
                })
            })
            .collect::<Result<_, Box<dyn std::error::Error>>>()?,
        allow_ranges: vec![],
        deny_ranges: vec![],
        denied_domains: vec![],
        opaque_tunnels: trogon_sandbox_contract::OpaqueTunnelPolicy::Allow,
        request_rules: vec![],
    })
}

fn facts(sni: &str) -> Result<FlowFacts, Box<dyn std::error::Error>> {
    Ok(FlowFacts {
        sni: Some(sni.to_string()),
        dest_ip: EDGE.parse()?,
        dest_port: 443,
        decider_allowed: false,
    })
}

async fn survives(guard: &mut FlowGuard) -> bool {
    timeout(Duration::from_millis(50), guard.cancelled()).await.is_err()
}

/// The shared-CDN case: two names live on one edge address. When the
/// policy stops allowing one, only its flow ends.
#[tokio::test]
async fn a_sweep_ends_the_flow_whose_name_is_no_longer_allowed_but_not_its_neighbor()
-> Result<(), Box<dyn std::error::Error>> {
    let flows = LiveFlows::new();
    let resolver = resolver(&[("allowed.example.com", EDGE), ("other.example.net", EDGE)])?;
    let mut allowed = flows.register(facts("allowed.example.com")?);
    let mut other = flows.register(facts("other.example.net")?);

    let cancelled = flows.sweep(&policy(&["allowed.example.com"])?, &resolver).await;

    assert_eq!(cancelled, 1);
    assert!(survives(&mut allowed).await);
    assert_eq!(
        timeout(Duration::from_millis(50), other.cancelled()).await?,
        EgressViolationReason::PolicyDenied
    );
    Ok(())
}

#[tokio::test]
async fn a_sweep_ends_a_flow_whose_name_no_longer_resolves_to_its_address() -> Result<(), Box<dyn std::error::Error>> {
    let flows = LiveFlows::new();
    let resolver = resolver(&[("allowed.example.com", "198.51.100.9")])?;
    let mut flow = flows.register(facts("allowed.example.com")?);

    let cancelled = flows.sweep(&policy(&["allowed.example.com"])?, &resolver).await;

    assert_eq!(cancelled, 1);
    assert_eq!(
        timeout(Duration::from_millis(50), flow.cancelled()).await?,
        EgressViolationReason::SniDestinationMismatch
    );
    Ok(())
}

#[tokio::test]
async fn an_unresolvable_name_keeps_a_flow_only_while_a_binding_is_live() -> Result<(), Box<dyn std::error::Error>> {
    let flows = LiveFlows::new();
    let resolver = resolver(&[])?;
    let mut unbound = flows.register(facts("allowed.example.com")?);
    assert_eq!(flows.sweep(&policy(&["allowed.example.com"])?, &resolver).await, 1);
    assert_eq!(
        timeout(Duration::from_millis(50), unbound.cancelled()).await?,
        EgressViolationReason::SniResolutionFailed
    );

    resolver
        .bindings()
        .record(&Host::new("allowed.example.com")?, EDGE.parse()?);
    let mut bound = flows.register(facts("allowed.example.com")?);
    assert_eq!(flows.sweep(&policy(&["allowed.example.com"])?, &resolver).await, 0);
    assert!(survives(&mut bound).await);
    Ok(())
}

#[tokio::test]
async fn a_decider_allowed_flow_is_judged_by_its_binding_alone() -> Result<(), Box<dyn std::error::Error>> {
    let flows = LiveFlows::new();
    let resolver = resolver(&[("asked.example.com", EDGE)])?;
    let mut flow = flows.register(FlowFacts {
        decider_allowed: true,
        ..facts("asked.example.com")?
    });
    assert_eq!(flows.sweep(&policy(&[])?, &resolver).await, 0);
    assert!(survives(&mut flow).await);
    Ok(())
}

#[tokio::test]
async fn dropping_the_guard_forgets_the_flow() -> Result<(), Box<dyn std::error::Error>> {
    let flows = LiveFlows::new();
    let guard = flows.register(facts("allowed.example.com")?);
    assert_eq!(flows.len(), 1);
    drop(guard);
    assert!(flows.is_empty());
    Ok(())
}

#[tokio::test]
async fn a_flow_admitted_without_a_name_is_left_alone() -> Result<(), Box<dyn std::error::Error>> {
    let flows = LiveFlows::new();
    let resolver = resolver(&[])?;
    let mut flow = flows.register(facts("allowed.example.com")?);
    assert_eq!(flows.sweep(&NetworkPolicy::AllowAll, &resolver).await, 0);
    assert!(survives(&mut flow).await);
    Ok(())
}

#[tokio::test]
async fn a_guard_whose_registry_entry_is_gone_never_resolves() -> Result<(), Box<dyn std::error::Error>> {
    let (cancel, cancelled) = watch::channel(None);
    drop(cancel);
    let mut guard = FlowGuard {
        flows: LiveFlows::new(),
        id: 0,
        cancelled,
    };
    assert!(survives(&mut guard).await);
    Ok(())
}
