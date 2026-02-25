use bytes::Bytes;
use std::time::{Duration, Instant};
use tracing::{info, warn};

use crate::traits::LeaderLock;

const RENEW_INTERVAL: Duration = Duration::from_secs(5);

pub struct LeaderElection<L: LeaderLock> {
    lock: L,
    node_id: String,
    is_leader: bool,
    last_renewed: Option<Instant>,
    current_revision: Option<u64>,
}

impl<L: LeaderLock> LeaderElection<L> {
    pub fn new(lock: L, node_id: String) -> Self {
        Self {
            lock,
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

    /// Release the lock on clean shutdown so the next leader is elected immediately.
    pub async fn release(&mut self) {
        if !self.is_leader {
            return;
        }
        match self.lock.release().await {
            Ok(_) => info!(node_id = %self.node_id, "Released CRON leader lock"),
            Err(e) => warn!(node_id = %self.node_id, error = %e, "Failed to release leader lock"),
        }
        self.is_leader = false;
        self.current_revision = None;
    }

    async fn try_acquire(&mut self) {
        let value = Bytes::from(self.node_id.clone());
        match self.lock.try_acquire(value).await {
            Ok(revision) => {
                self.is_leader = true;
                self.current_revision = Some(revision);
                self.last_renewed = Some(Instant::now());
                info!(node_id = %self.node_id, "Became CRON leader");
            }
            Err(_) => {
                // Another node holds the lock — expected, not an error.
            }
        }
    }

    async fn maybe_renew(&mut self) {
        let should_renew = self
            .last_renewed
            .is_none_or(|t| t.elapsed() >= RENEW_INTERVAL);

        if !should_renew {
            return;
        }

        let Some(revision) = self.current_revision else {
            self.is_leader = false;
            return;
        };

        let value = Bytes::from(self.node_id.clone());
        match self.lock.renew(value, revision).await {
            Ok(new_rev) => {
                self.current_revision = Some(new_rev);
                self.last_renewed = Some(Instant::now());
            }
            Err(e) => {
                warn!(node_id = %self.node_id, error = %e, "Lost CRON leadership (renew failed)");
                self.is_leader = false;
                self.current_revision = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "test-support")]
    mod with_mocks {
        use super::super::*;
        use crate::mocks::MockLeaderLock;

        #[tokio::test]
        async fn acquires_leadership_when_lock_is_free() {
            let lock = MockLeaderLock::new();
            let mut election = LeaderElection::new(lock, "node-1".to_string());

            assert!(!election.is_leader());
            let became_leader = election.ensure_leader().await;
            assert!(became_leader);
            assert!(election.is_leader());
        }

        #[tokio::test]
        async fn stays_follower_when_lock_is_denied() {
            let lock = MockLeaderLock::new();
            lock.deny_acquire();
            let mut election = LeaderElection::new(lock, "node-2".to_string());

            let became_leader = election.ensure_leader().await;
            assert!(!became_leader);
            assert!(!election.is_leader());
        }

        #[tokio::test]
        async fn loses_leadership_when_renew_fails() {
            let lock = MockLeaderLock::new();
            let mut election = LeaderElection::new(lock.clone(), "node-3".to_string());

            // Acquire leadership
            election.ensure_leader().await;
            assert!(election.is_leader());

            // Force immediate renew by clearing last_renewed
            election.last_renewed = None;

            // Deny renewal — simulates lost leadership (e.g. clock skew, NATS partition)
            lock.deny_renew();
            election.ensure_leader().await;
            assert!(!election.is_leader());
        }

        #[tokio::test]
        async fn release_clears_leader_state() {
            let lock = MockLeaderLock::new();
            let mut election = LeaderElection::new(lock, "node-4".to_string());
            election.ensure_leader().await;
            assert!(election.is_leader());

            election.release().await;
            assert!(!election.is_leader());
        }

        #[tokio::test]
        async fn renewal_is_throttled_within_renew_interval() {
            let lock = MockLeaderLock::new();
            let mut election = LeaderElection::new(lock.clone(), "node-throttle".to_string());

            // Acquire leadership — sets last_renewed to now.
            election.ensure_leader().await;
            assert!(election.is_leader());

            // Deny renew: if throttle is broken, the next ensure_leader call will
            // attempt to renew and fail, losing leadership. If throttle works, it
            // skips the renew entirely and we stay leader.
            lock.deny_renew();
            election.ensure_leader().await;
            assert!(election.is_leader(), "renewal must be throttled within RENEW_INTERVAL");
        }

        #[tokio::test]
        async fn loses_leadership_when_revision_is_none() {
            let lock = MockLeaderLock::new();
            let mut election = LeaderElection::new(lock, "node-norev".to_string());

            // Construct an inconsistent leader state: is_leader=true but no revision.
            election.is_leader = true;
            election.last_renewed = None;     // force a renewal attempt
            election.current_revision = None; // but no revision is stored

            election.ensure_leader().await;
            assert!(!election.is_leader(), "missing revision must clear leadership");
        }

        #[tokio::test]
        async fn release_when_not_leader_is_noop() {
            let lock = MockLeaderLock::new();
            lock.deny_acquire();
            let mut election = LeaderElection::new(lock, "node-nop".to_string());

            election.ensure_leader().await;
            assert!(!election.is_leader());

            // Must not panic; state must remain unchanged.
            election.release().await;
            assert!(!election.is_leader());
        }

        #[tokio::test]
        async fn revision_is_cleared_after_release() {
            let lock = MockLeaderLock::new();
            let mut election = LeaderElection::new(lock, "node-rev".to_string());

            election.ensure_leader().await;
            assert!(election.current_revision.is_some(), "revision must be set after acquire");

            election.release().await;
            assert!(election.current_revision.is_none(), "revision must be cleared after release");
            assert!(!election.is_leader());
        }
    }
}
