use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use trogon_sandbox_contract::{LeaseEpoch, LeaseId, WorkerAuthorityRequest, WorkerAuthorityResponse, WorkerId};

use crate::worker_authority_constants::MAX_LEASES_PER_REQUEST;
use crate::{
    AuthorityConfig, AuthorityHost, AuthorityMode, AuthorityOracle, AuthorityStats, FreezeOutcome, HeldLease,
    OracleError, PurgeOutcome, ThawOutcome,
};

#[derive(Debug)]
struct Suspect {
    since: Instant,
    freeze: FreezeState,
    reclaiming: bool,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum FreezeState {
    NoTransition,
    Owned,
    Uncertain,
}

/// Periodically asks the broker whether the worker still owns what it holds, then freezes, thaws and purges.
pub struct AuthorityGuard {
    config: AuthorityConfig,
    oracle: Arc<dyn AuthorityOracle>,
    host: Arc<dyn AuthorityHost>,
    worker_id: WorkerId,
    stats: Arc<AuthorityStats>,
    suspects: HashMap<HeldLease, Suspect>,
    first_seen: HashMap<HeldLease, Instant>,
    strikes: HashMap<HeldLease, u32>,
    warned_no_responders: bool,
    last_success: Instant,
}

impl AuthorityGuard {
    /// A guard for `worker_id` that asks `oracle` and acts on `host`.
    pub fn new(
        config: AuthorityConfig,
        worker_id: WorkerId,
        oracle: Arc<dyn AuthorityOracle>,
        host: Arc<dyn AuthorityHost>,
    ) -> Self {
        Self {
            config,
            oracle,
            host,
            worker_id,
            stats: Arc::new(AuthorityStats::default()),
            suspects: HashMap::new(),
            first_seen: HashMap::new(),
            strikes: HashMap::new(),
            warned_no_responders: false,
            last_success: Instant::now(),
        }
    }

    /// The counters this guard updates.
    pub fn stats(&self) -> Arc<AuthorityStats> {
        Arc::clone(&self.stats)
    }

    /// Checks on a timer until `cancel` fires. Does nothing in `off` mode.
    pub async fn run(mut self, cancel: CancellationToken) {
        if self.config.mode == AuthorityMode::Off {
            return;
        }
        let mut ticker = tokio::time::interval(self.config.interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = ticker.tick() => self.tick().await,
                _ = cancel.cancelled() => break,
            }
        }
    }

    /// One check: ask the broker, then freeze, thaw and purge as the answer and the clock call for.
    pub async fn tick(&mut self) {
        if self.config.mode == AuthorityMode::Off {
            return;
        }
        let now = Instant::now();
        let held = match self.host.held_leases().await {
            Ok(held) => held,
            Err(error) => {
                AuthorityStats::bump(&self.stats.check_failures);
                tracing::warn!(%error, "worker authority inventory failed; retaining pending ownership");
                return;
            }
        };
        // Equality and hashing of a held lease never read the atomic flag inside its completion handle.
        #[allow(clippy::mutable_key_type)]
        let held_set: HashSet<HeldLease> = held.iter().cloned().collect();
        self.first_seen.retain(|lease, _| held_set.contains(lease));
        self.strikes.retain(|lease, _| held_set.contains(lease));
        for lease in &held {
            self.first_seen.entry(lease.clone()).or_insert(now);
        }
        let mut pending = held;
        for lease in self.suspects.keys() {
            if !held_set.contains(lease) {
                pending.push(lease.clone());
            }
        }
        if pending.is_empty() {
            self.last_success = now;
            return;
        }

        match self.ask(&pending).await {
            Ok(response) => {
                self.last_success = now;
                self.apply_answer(&pending, &response, now).await;
            }
            Err(OracleError::NoResponders) => {
                AuthorityStats::bump(&self.stats.no_responders);
                if !self.warned_no_responders {
                    self.warned_no_responders = true;
                    tracing::warn!(
                        worker_id = %self.worker_id,
                        mode = ?self.config.mode,
                        "no broker answers the lease authority check"
                    );
                }
                if self.config.mode != AuthorityMode::Enforce {
                    self.last_success = now;
                    return;
                }
                self.apply_partition(&pending, now).await;
            }
            Err(error) => {
                AuthorityStats::bump(&self.stats.check_failures);
                let silent_for = now.saturating_duration_since(self.last_success);
                tracing::warn!(
                    worker_id = %self.worker_id,
                    %error,
                    silent_secs = silent_for.as_secs(),
                    "lease authority check failed"
                );
                self.apply_partition(&pending, now).await;
            }
        }
        self.purge_expired(now).await;
    }

    async fn apply_partition(&mut self, held: &[HeldLease], now: Instant) {
        if now.saturating_duration_since(self.last_success) < self.config.partition_after {
            return;
        }
        for lease in held {
            if self.suspects.contains_key(lease) {
                self.still_suspect(lease, now).await;
            } else {
                AuthorityStats::bump(&self.stats.partitioned);
                self.suspect(lease, "broker authority unavailable", now).await;
            }
        }
    }

    async fn ask(&self, held: &[HeldLease]) -> Result<WorkerAuthorityResponse, OracleError> {
        let mut ids: Vec<LeaseId> = held.iter().map(|lease| lease.lease_id().clone()).collect();
        ids.sort_by(|a, b| a.as_str().cmp(b.as_str()));
        ids.dedup();
        let mut combined = WorkerAuthorityResponse::default();
        for chunk in ids.chunks(MAX_LEASES_PER_REQUEST) {
            let request = WorkerAuthorityRequest {
                worker_id: self.worker_id.clone(),
                lease_ids: chunk.to_vec(),
            };
            let answer = tokio::time::timeout(self.config.request_timeout, self.oracle.owned(&request))
                .await
                .map_err(|_| OracleError::TimedOut)??;
            combined.owned.extend(answer.owned);
            combined.not_assigned.extend(answer.not_assigned);
        }
        Ok(combined)
    }

    async fn apply_answer(&mut self, held: &[HeldLease], response: &WorkerAuthorityResponse, now: Instant) {
        let mut owned: HashMap<LeaseId, Option<Option<LeaseEpoch>>> = HashMap::new();
        for entry in &response.owned {
            owned
                .entry(entry.lease_id.clone())
                .and_modify(|proof| *proof = None)
                .or_insert(Some(entry.epoch));
        }
        for lease in held {
            let confirmed = match owned.get(lease.lease_id()) {
                Some(Some(epoch)) if !response.not_assigned.contains(lease.lease_id()) => match lease {
                    HeldLease::Targeted(identity) => {
                        identity.target.worker_id == self.worker_id
                            && identity.target.epoch.is_some()
                            && identity.target.epoch == *epoch
                    }
                    HeldLease::Provider { .. } => epoch.is_none(),
                },
                _ => false,
            };
            if confirmed {
                self.strikes.remove(lease);
                self.reclaimed(lease).await;
                continue;
            }
            if self.suspects.contains_key(lease) {
                self.still_suspect(lease, now).await;
                continue;
            }
            // A lease the broker has not had time to record looks the same as one it dropped, so a young lease is
            // not judged at all.
            let first_seen = self.first_seen.get(lease).copied().unwrap_or(now);
            let age = now.saturating_duration_since(first_seen);
            if age < self.config.min_lease_age {
                continue;
            }
            let strikes = self.strikes.entry(lease.clone()).or_insert(0);
            *strikes += 1;
            if *strikes < self.config.confirmations {
                continue;
            }
            self.strikes.remove(lease);
            AuthorityStats::bump(&self.stats.unowned);
            let reason = if response.not_assigned.contains(lease.lease_id()) {
                "broker assigns it to another worker or has finished it"
            } else {
                "broker does not know the lease"
            };
            self.suspect(lease, reason, now).await;
        }
    }

    async fn suspect(&mut self, lease: &HeldLease, reason: &str, now: Instant) {
        let freeze = self.freeze(lease).await;
        let frozen = freeze == FreezeState::Owned;
        tracing::warn!(metric = "sandbox_worker_authority_suspect_total", worker_id = %self.worker_id,
            lease_id = %lease.lease_id().as_str(), reason, mode = ?self.config.mode, frozen,
            "worker holds a lease it may not own");
        self.suspects.insert(
            lease.clone(),
            Suspect {
                since: now,
                freeze,
                reclaiming: false,
            },
        );
    }

    async fn freeze(&self, lease: &HeldLease) -> FreezeState {
        if self.config.mode != AuthorityMode::Enforce {
            return FreezeState::NoTransition;
        }
        match self.host.freeze(lease).await {
            Ok(FreezeOutcome::Frozen) => {
                AuthorityStats::bump(&self.stats.frozen);
                FreezeState::Owned
            }
            Ok(FreezeOutcome::Stale) => FreezeState::Uncertain,
            Ok(FreezeOutcome::AlreadyPaused | FreezeOutcome::Unsupported) => FreezeState::NoTransition,
            Err(error) => {
                tracing::warn!(lease_id = %lease.lease_id().as_str(), %error,
                    "freeze outcome is uncertain; retaining possible transition ownership");
                FreezeState::Uncertain
            }
        }
    }

    async fn still_suspect(&mut self, lease: &HeldLease, now: Instant) {
        let Some(suspect) = self.suspects.get_mut(lease) else {
            return;
        };
        if suspect.reclaiming {
            suspect.reclaiming = false;
            suspect.since = now;
        }
        if suspect.freeze == FreezeState::Uncertain {
            let freeze = self.freeze(lease).await;
            if let Some(suspect) = self.suspects.get_mut(lease) {
                suspect.freeze = freeze;
            }
        }
    }

    async fn reclaimed(&mut self, lease: &HeldLease) {
        let Some(suspect) = self.suspects.get_mut(lease) else {
            return;
        };
        if suspect.freeze == FreezeState::NoTransition {
            self.suspects.remove(lease);
            return;
        }
        suspect.reclaiming = true;
        match self.host.thaw(lease).await {
            Ok(ThawOutcome::Thawed) => {
                AuthorityStats::bump(&self.stats.thawed);
                self.suspects.remove(lease);
            }
            Ok(ThawOutcome::NoLongerOwned) => {
                self.suspects.remove(lease);
            }
            Ok(ThawOutcome::Stale) => {}
            Err(error) => tracing::warn!(lease_id = %lease.lease_id().as_str(), %error,
                "thawing a reclaimed lease failed; retaining transition ownership"),
        }
    }

    async fn purge_expired(&mut self, now: Instant) {
        if self.config.mode != AuthorityMode::Enforce {
            return;
        }
        let mut expired: Vec<HeldLease> = self
            .suspects
            .iter()
            .filter(|(_, suspect)| {
                !suspect.reclaiming && now.saturating_duration_since(suspect.since) >= self.config.grace
            })
            .map(|(lease, _)| lease.clone())
            .collect();
        expired.sort_by(|a, b| {
            a.lease_id()
                .as_str()
                .cmp(b.lease_id().as_str())
                .then_with(|| format!("{a:?}").cmp(&format!("{b:?}")))
        });
        for lease in expired {
            match self.host.purge(&lease).await {
                Ok(PurgeOutcome::Purged) => {
                    self.suspects.remove(&lease);
                    AuthorityStats::bump(&self.stats.purged);
                }
                Ok(PurgeOutcome::Settled) => {
                    self.suspects.remove(&lease);
                }
                Ok(PurgeOutcome::Stale) => {}
                Err(error) => tracing::warn!(lease_id = %lease.lease_id().as_str(), %error,
                    "purging an unowned lease failed; retaining exact cleanup ownership"),
            }
        }
    }
}
