//! The flows the transparent listener is carrying right now, so a policy
//! replacement can revalidate them.
//!
//! A flow is admitted once, at connect time. Without this registry a later
//! policy change only affects new connections. A sweep re-asks, for each live
//! flow, whether the new policy still admits it and, where the admission
//! rested on the name the client presented, whether the proxy's own
//! resolution still binds that name to the address the flow is using. A flow
//! that fails is cancelled; another flow on the same shared address whose name
//! is still allowed and bound is left alone.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};

use tokio::sync::watch;
use trogon_sandbox_contract::{EgressViolationReason, NetworkPolicy};
use trogon_sandbox_contract::{Host, Port};

use crate::policy::{Ruling, allow_rests_on_sni, rule};
use crate::resolve::{NameBindingError, VettingResolver};

/// What a sweep needs to know about an admitted flow.
#[derive(Debug, Clone)]
pub struct FlowFacts {
    pub sni: Option<String>,
    pub dest_ip: IpAddr,
    pub dest_port: u16,
    /// A decider, not the policy, allowed the name. The policy cannot be
    /// asked again, so only the binding is revalidated.
    pub decider_allowed: bool,
}

struct Entry {
    facts: FlowFacts,
    cancel: watch::Sender<Option<EgressViolationReason>>,
}

#[derive(Default)]
pub struct LiveFlows {
    next: Mutex<(u64, HashMap<u64, Entry>)>,
}

/// Held for the life of a flow; dropping it removes the flow from the
/// registry.
pub struct FlowGuard {
    flows: Arc<LiveFlows>,
    id: u64,
    cancelled: watch::Receiver<Option<EgressViolationReason>>,
}

impl FlowGuard {
    /// Resolves with the reason once a sweep cancels this flow. Never resolves
    /// for a flow that is not cancelled.
    pub async fn cancelled(&mut self) -> EgressViolationReason {
        loop {
            if let Some(reason) = *self.cancelled.borrow_and_update() {
                return reason;
            }
            if self.cancelled.changed().await.is_err() {
                std::future::pending::<()>().await;
            }
        }
    }
}

impl Drop for FlowGuard {
    fn drop(&mut self) {
        self.flows.lock().1.remove(&self.id);
    }
}

impl LiveFlows {
    pub fn new() -> Arc<Self> {
        Arc::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, (u64, HashMap<u64, Entry>)> {
        self.next.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn register(self: &Arc<Self>, facts: FlowFacts) -> FlowGuard {
        let (cancel, cancelled) = watch::channel(None);
        let mut state = self.lock();
        state.0 += 1;
        let id = state.0;
        state.1.insert(id, Entry { facts, cancel });
        FlowGuard {
            flows: Arc::clone(self),
            id,
            cancelled,
        }
    }

    pub fn len(&self) -> usize {
        self.lock().1.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Cancels every live flow `policy` no longer admits, or whose
    /// name-based admission the proxy can no longer back with an observed
    /// binding. `resolver` is asked afresh, so a name that still resolves to
    /// the flow's address keeps the flow (and refreshes the binding), while a
    /// name that no longer does, or that cannot be resolved without a live
    /// binding already on record, ends it. Returns how many were cancelled.
    pub async fn sweep(&self, policy: &NetworkPolicy, resolver: &VettingResolver) -> usize {
        let snapshot: Vec<(u64, FlowFacts)> = self
            .lock()
            .1
            .iter()
            .map(|(id, entry)| (*id, entry.facts.clone()))
            .collect();
        let mut cancelled = 0;
        for (id, facts) in snapshot {
            if let Some(reason) = Self::verdict(policy, resolver, &facts).await
                && let Some(entry) = self.lock().1.get(&id)
            {
                let _ = entry.cancel.send(Some(reason));
                cancelled += 1;
            }
        }
        cancelled
    }

    async fn verdict(
        policy: &NetworkPolicy,
        resolver: &VettingResolver,
        facts: &FlowFacts,
    ) -> Option<EgressViolationReason> {
        let sni = facts.sni.as_deref();
        if !facts.decider_allowed {
            match rule(policy, sni, facts.dest_ip, facts.dest_port) {
                Ruling::Allow(_) => {}
                Ruling::Deny | Ruling::Unlisted => {
                    return Some(EgressViolationReason::PolicyDenied);
                }
            }
        }
        let rests_on_name = facts.decider_allowed || allow_rests_on_sni(policy, sni, facts.dest_ip, facts.dest_port);
        if !rests_on_name {
            return None;
        }
        let name = Host::new(sni?).ok()?;
        let port = Port::new(facts.dest_port).ok()?;
        match resolver.confirm_binding(&name, port, facts.dest_ip).await {
            Ok(()) => None,
            Err(NameBindingError::NotAnAnswer) => Some(EgressViolationReason::SniDestinationMismatch),
            Err(NameBindingError::CarriesSentinel) => Some(EgressViolationReason::PolicyDenied),
            Err(NameBindingError::Unresolved(_)) => resolver
                .bindings()
                .is_bound(&name, facts.dest_ip)
                .then_some(())
                .map_or(Some(EgressViolationReason::SniResolutionFailed), |()| None),
        }
    }
}

#[cfg(test)]
mod tests;
