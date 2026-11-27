use super::*;

/// Operator limits on how long one lease may live.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct LifetimePolicy {
    /// Longest total lifetime, initial `ttl_seconds` plus every extension.
    pub max_lease_lifetime_seconds: u32,
    /// Whether a lease may ask for `no_ttl`. Off unless the operator opts in,
    /// because such a lease holds its sandbox until released or idle.
    pub allow_no_ttl: bool,
}

impl Default for LifetimePolicy {
    fn default() -> Self {
        Self {
            max_lease_lifetime_seconds: MAX_LEASE_TTL_SECONDS,
            allow_no_ttl: false,
        }
    }
}

impl LifetimePolicy {
    /// How far ahead a no-TTL lease's deadline sits. Far enough that only the
    /// idle limit or a release ends it, near enough that `SystemTime` math and
    /// every store's timestamp encoding stay in range.
    pub const NO_TTL_DEADLINE_OFFSET: Duration = Duration::from_secs(100 * 365 * 24 * 60 * 60);

    pub(super) fn check(&self, req: &LeaseRequest) -> Result<(), LeaseError> {
        if req.spec.no_ttl().is_unbounded() {
            if !self.allow_no_ttl {
                return Err(LeaseError::InvalidSpec {
                    reason: "no_ttl leases are not enabled on this broker".to_string(),
                });
            }
            let idle_release = req.spec.idle_policy().is_some_and(|policy| {
                policy
                    .stages()
                    .iter()
                    .any(|stage| stage.action == trogon_sandbox_contract::IdleAction::Release)
            });
            if req.spec.idle_timeout_seconds().is_none() && !idle_release {
                return Err(LeaseError::InvalidSpec {
                    reason: "no_ttl needs idle_timeout_seconds or an idle_policy release stage".to_string(),
                });
            }
        }
        if req.ttl_seconds > self.max_lease_lifetime_seconds {
            return Err(LeaseError::WouldExceedMaxLifetime {
                max: self.max_lease_lifetime_seconds,
            });
        }
        Ok(())
    }
}
