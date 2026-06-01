use std::sync::Arc;
use std::time::Duration;

use super::caller_budget::{CallerBudgetRegistry, CallerKey};
use super::inflight::{InflightRegistry, InflightReleaseKey, TENANT_INFLIGHT_RETRY_AFTER_MS};

/// RAII guard; releases tenant and server inflight slots on drop (success, error, or timeout).
pub struct InflightGuard {
    limiter: Arc<RateLimiter>,
    server_key: Option<InflightReleaseKey>,
    tenant_key: Option<InflightReleaseKey>,
}

impl Drop for InflightGuard {
    fn drop(&mut self) {
        if let Some(ref key) = self.server_key {
            self.limiter.server_inflight.release(&key.0);
        }
        if let Some(ref key) = self.tenant_key {
            self.limiter.tenant_inflight.release(&key.0);
        }
    }
}

/// Wire-Format Pin 9 production defaults (overridable for tests).
#[derive(Clone, Debug)]
pub struct RateLimitConfig {
    pub enabled: bool,
    pub server_inflight_max: u32,
    pub tenant_inflight_max: u32,
    pub caller_budget: u32,
    pub caller_window: Duration,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            server_inflight_max: 256,
            tenant_inflight_max: 4096,
            caller_budget: 100,
            caller_window: Duration::from_secs(10),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RateLimitScope {
    Server,
    Tenant,
    Caller,
    Purpose,
}

impl RateLimitScope {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Server => "server",
            Self::Tenant => "tenant",
            Self::Caller => "caller",
            Self::Purpose => "purpose",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RateLimitDeny {
    pub scope: RateLimitScope,
    pub retry_after_ms: u64,
}

#[derive(Debug)]
pub struct RateLimiter {
    config: RateLimitConfig,
    server_inflight: InflightRegistry,
    tenant_inflight: InflightRegistry,
    caller_budget: CallerBudgetRegistry,
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self::new(RateLimitConfig::default())
    }
}

impl RateLimiter {
    #[must_use]
    pub fn new(config: RateLimitConfig) -> Self {
        let caller_budget =
            CallerBudgetRegistry::new(config.caller_window, config.caller_budget);
        Self {
            config,
            server_inflight: InflightRegistry::default(),
            tenant_inflight: InflightRegistry::default(),
            caller_budget,
        }
    }

    #[must_use]
    pub fn config(&self) -> &RateLimitConfig {
        &self.config
    }

    /// Post-auth caller rate check (Tier 2 in-process; KV sync TODO per ADR 0012).
    pub fn check_caller(&self, tenant: &str, sub: &str) -> Option<RateLimitDeny> {
        if !self.config.enabled {
            return None;
        }
        let key = CallerKey {
            tenant: tenant.to_string(),
            sub: sub.to_string(),
        };
        self.caller_budget.try_consume(&key).err().map(|retry_after_ms| RateLimitDeny {
            scope: RateLimitScope::Caller,
            retry_after_ms,
        })
    }

    /// Acquire tenant then server inflight slots; releases tenant on server deny.
    pub fn try_acquire_inflight(
        self: &Arc<Self>,
        server_id: &str,
        tenant: &str,
    ) -> Result<InflightGuard, RateLimitDeny> {
        if !self.config.enabled {
            return Ok(InflightGuard {
                limiter: self.clone(),
                server_key: None,
                tenant_key: None,
            });
        }

        let tenant_key = match self.tenant_inflight.try_acquire(
            tenant,
            self.config.tenant_inflight_max,
        ) {
            Ok(key) => Some(key),
            Err(_) => {
                return Err(RateLimitDeny {
                    scope: RateLimitScope::Tenant,
                    retry_after_ms: TENANT_INFLIGHT_RETRY_AFTER_MS,
                });
            }
        };

        match self
            .server_inflight
            .try_acquire(server_id, self.config.server_inflight_max)
        {
            Ok(server_key) => Ok(InflightGuard {
                limiter: self.clone(),
                server_key: Some(server_key),
                tenant_key,
            }),
            Err(retry_after_ms) => {
                if let Some(ref tk) = tenant_key {
                    self.tenant_inflight.release(&tk.0);
                }
                Err(RateLimitDeny {
                    scope: RateLimitScope::Server,
                    retry_after_ms,
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_limiter() -> Arc<RateLimiter> {
        Arc::new(RateLimiter::new(RateLimitConfig {
            enabled: true,
            server_inflight_max: 2,
            tenant_inflight_max: 2,
            caller_budget: 3,
            caller_window: Duration::from_secs(1),
        }))
    }

    #[test]
    fn inflight_guard_releases_on_drop() {
        let limiter = test_limiter();
        let g1 = limiter.try_acquire_inflight("srv", "acme").unwrap();
        let _g2 = limiter.try_acquire_inflight("srv", "acme").unwrap();
        assert!(limiter.try_acquire_inflight("srv", "acme").is_err());
        drop(g1);
        assert!(limiter.try_acquire_inflight("srv", "acme").is_ok());
    }

    #[test]
    fn tenant_cap_applies_across_servers() {
        let limiter = test_limiter();
        let _g1 = limiter.try_acquire_inflight("srv-a", "acme").unwrap();
        let _g2 = limiter.try_acquire_inflight("srv-b", "acme").unwrap();
        let deny = match limiter.try_acquire_inflight("srv-c", "acme") {
            Err(d) => d,
            Ok(_) => panic!("expected tenant inflight deny"),
        };
        assert_eq!(deny.scope, RateLimitScope::Tenant);
    }

    #[test]
    fn distinct_tenants_isolated_inflight() {
        let limiter = test_limiter();
        let _g1 = limiter.try_acquire_inflight("srv-a", "tenant-a").unwrap();
        let _g2 = limiter.try_acquire_inflight("srv-b", "tenant-a").unwrap();
        assert!(limiter.try_acquire_inflight("srv-c", "tenant-a").is_err());
        assert!(limiter.try_acquire_inflight("srv-d", "tenant-b").is_ok());
    }

    #[test]
    fn caller_budget_isolated_by_sub() {
        let limiter = test_limiter();
        for _ in 0..3 {
            assert!(limiter.check_caller("acme", "alice").is_none());
        }
        assert!(limiter.check_caller("acme", "alice").is_some());
        assert!(limiter.check_caller("acme", "bob").is_none());
    }
}
