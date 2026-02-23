use async_nats::jetstream::kv;
use bytes::Bytes;
use std::time::{Duration, Instant};
use tracing::{info, warn};

const RENEW_INTERVAL: Duration = Duration::from_secs(5);

pub struct LeaderElection {
    kv: kv::Store,
    node_id: String,
    is_leader: bool,
    last_renewed: Option<Instant>,
    current_revision: Option<u64>,
}

impl LeaderElection {
    pub fn new(kv: kv::Store, node_id: String) -> Self {
        Self {
            kv,
            node_id,
            is_leader: false,
            last_renewed: None,
            current_revision: None,
        }
    }

    pub fn is_leader(&self) -> bool {
        self.is_leader
    }

    /// Tries to acquire or renew leadership. Returns true if this node is the leader.
    pub async fn ensure_leader(&mut self) -> bool {
        if self.is_leader {
            self.maybe_renew().await;
        } else {
            self.try_acquire().await;
        }
        self.is_leader
    }

    /// Release the lock on clean shutdown so the next leader is elected immediately
    /// instead of waiting for the TTL to expire.
    pub async fn release(&mut self) {
        if !self.is_leader {
            return;
        }
        match self.kv.delete(crate::kv::LEADER_KEY).await {
            Ok(_) => info!(node_id = %self.node_id, "Released CRON leader lock"),
            Err(e) => warn!(node_id = %self.node_id, error = %e, "Failed to release leader lock"),
        }
        self.is_leader = false;
        self.current_revision = None;
    }

    async fn try_acquire(&mut self) {
        let value = Bytes::from(self.node_id.clone());
        match self.kv.create(crate::kv::LEADER_KEY, value).await {
            Ok(revision) => {
                self.is_leader = true;
                self.current_revision = Some(revision);
                self.last_renewed = Some(Instant::now());
                info!(node_id = %self.node_id, "Became CRON leader");
            }
            Err(_) => {
                // Another node holds the lock — this is expected, not an error.
            }
        }
    }

    async fn maybe_renew(&mut self) {
        let should_renew = self
            .last_renewed
            .map_or(true, |t| t.elapsed() >= RENEW_INTERVAL);

        if !should_renew {
            return;
        }

        let Some(revision) = self.current_revision else {
            self.is_leader = false;
            return;
        };

        let value = Bytes::from(self.node_id.clone());
        match self.kv.update(crate::kv::LEADER_KEY, value, revision).await {
            Ok(new_rev) => {
                self.current_revision = Some(new_rev);
                self.last_renewed = Some(Instant::now());
            }
            Err(e) => {
                warn!(node_id = %self.node_id, error = %e, "Lost CRON leadership (update failed)");
                self.is_leader = false;
                self.current_revision = None;
            }
        }
    }
}
