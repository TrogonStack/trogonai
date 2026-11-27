//! Periodic revalidation of open flows, apart from any policy change.

use std::collections::HashMap;
use std::io;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::watch;
use tokio::time::timeout;
use trogon_sandbox_contract::Port;
use trogon_sandbox_contract::{DomainRule, DomainRuleAction, EgressViolationReason, NetworkPolicy};

use crate::live_flows::FlowFacts;
use crate::resolve::{NameLookup, SsrfFloor, VettingResolver};
use crate::server::{PolicyHandle, revalidate_periodically};
use crate::substitution_tests::TestResult;
use crate::tls_ca::PermittedDomains;

const NAME: &str = "api.example.com";
const OLD: &str = "203.0.113.7";
const MOVED: &str = "198.51.100.9";

struct Table(Mutex<HashMap<String, Vec<IpAddr>>>);

#[async_trait::async_trait]
impl NameLookup for Table {
    async fn lookup(&self, name: &str, _port: Port) -> io::Result<Vec<IpAddr>> {
        let table = self.0.lock().map_err(|_| io::Error::other("poisoned lock"))?;
        table.get(name).cloned().ok_or_else(|| io::ErrorKind::NotFound.into())
    }
}

fn policy() -> TestResult<NetworkPolicy> {
    Ok(NetworkPolicy::UserDefined {
        rules: vec![DomainRule {
            domain: NAME.parse()?,
            action: DomainRuleAction::PassThrough,
        }],
        allow_ranges: vec![],
        deny_ranges: vec![],
        denied_domains: vec![],
        opaque_tunnels: trogon_sandbox_contract::OpaqueTunnelPolicy::Allow,
        request_rules: vec![],
    })
}

fn flow() -> TestResult<FlowFacts> {
    Ok(FlowFacts {
        sni: Some(NAME.to_string()),
        dest_ip: OLD.parse()?,
        dest_port: 443,
        decider_allowed: false,
    })
}

fn point(table: &Arc<Table>, ip: &str) -> TestResult<()> {
    let address: IpAddr = ip.parse()?;
    table
        .0
        .lock()
        .map_err(|_| "poisoned lock")?
        .insert(NAME.to_string(), vec![address]);
    Ok(())
}

fn start(table: &Arc<Table>, interval: Duration) -> TestResult<(PolicyHandle, watch::Sender<Option<Duration>>)> {
    let handle = PolicyHandle::new(policy()?, PermittedDomains::from_rules(&[])?);
    let resolver = Arc::new(VettingResolver::new(table.clone(), SsrfFloor::system()));
    let (shutdown, signal) = watch::channel(None);
    tokio::spawn(revalidate_periodically(handle.clone(), resolver, interval, signal));
    Ok((handle, shutdown))
}

#[tokio::test]
async fn a_flow_whose_name_moved_to_another_address_ends_without_a_policy_change()
-> Result<(), Box<dyn std::error::Error>> {
    let table = Arc::new(Table(Mutex::default()));
    point(&table, OLD)?;
    let (handle, _shutdown) = start(&table, Duration::from_millis(40))?;
    let mut guard = handle.live_flows().register(flow()?);

    point(&table, MOVED)?;

    let reason = timeout(Duration::from_secs(5), guard.cancelled()).await?;
    assert_eq!(reason, EgressViolationReason::SniDestinationMismatch);
    Ok(())
}

#[tokio::test]
async fn a_flow_whose_name_still_resolves_to_its_address_is_left_alone() -> Result<(), Box<dyn std::error::Error>> {
    let table = Arc::new(Table(Mutex::default()));
    point(&table, OLD)?;
    let (handle, _shutdown) = start(&table, Duration::from_millis(20))?;
    let mut guard = handle.live_flows().register(flow()?);

    assert!(timeout(Duration::from_millis(300), guard.cancelled()).await.is_err());
    Ok(())
}

#[tokio::test]
async fn shutdown_stops_the_sweeper() -> Result<(), Box<dyn std::error::Error>> {
    let table = Arc::new(Table(Mutex::default()));
    point(&table, OLD)?;
    let handle = PolicyHandle::new(policy()?, PermittedDomains::from_rules(&[])?);
    let resolver = Arc::new(VettingResolver::new(table, SsrfFloor::system()));
    let (shutdown, signal) = watch::channel(None);
    let task = tokio::spawn(revalidate_periodically(
        handle,
        resolver,
        Duration::from_millis(20),
        signal,
    ));
    shutdown.send(Some(Duration::ZERO))?;
    timeout(Duration::from_secs(5), task).await???;
    Ok(())
}
