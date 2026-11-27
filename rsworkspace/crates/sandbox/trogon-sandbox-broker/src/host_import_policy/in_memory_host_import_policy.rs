use std::collections::HashMap;

use async_trait::async_trait;
use trogon_sandbox_contract::{HostImport, LeaseError, TenantId};

use super::{HostImportPolicy, TenantHostImportPolicy};

pub struct InMemoryHostImportPolicy {
    policies: tokio::sync::Mutex<HashMap<TenantId, TenantHostImportPolicy>>,
}

impl InMemoryHostImportPolicy {
    pub fn new() -> Self {
        Self {
            policies: tokio::sync::Mutex::new(HashMap::new()),
        }
    }

    pub async fn insert(&self, tenant: &TenantId, policy: TenantHostImportPolicy) {
        self.policies.lock().await.insert(tenant.clone(), policy);
    }
}

impl Default for InMemoryHostImportPolicy {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl HostImportPolicy for InMemoryHostImportPolicy {
    async fn validate(&self, tenant: &TenantId, requested: &[HostImport]) -> Result<(), LeaseError> {
        if requested.is_empty() {
            return Ok(());
        }

        let policies = self.policies.lock().await;
        let policy = match policies.get(tenant) {
            Some(p) => p,
            None => {
                return Err(LeaseError::InvalidSpec {
                    reason: format!("host imports not permitted for tenant '{tenant}'"),
                });
            }
        };

        for import in requested {
            match import {
                HostImport::FetchUrl {
                    allow_domains,
                    max_body_bytes,
                    timeout_seconds,
                } => {
                    if *max_body_bytes > policy.max_body_bytes {
                        return Err(LeaseError::InvalidSpec {
                            reason: format!(
                                "fetch_url max_body_bytes {max_body_bytes} exceeds tenant cap {}",
                                policy.max_body_bytes
                            ),
                        });
                    }
                    if *timeout_seconds > policy.max_timeout_seconds {
                        return Err(LeaseError::InvalidSpec {
                            reason: format!(
                                "fetch_url timeout_seconds {timeout_seconds} exceeds tenant cap {}",
                                policy.max_timeout_seconds
                            ),
                        });
                    }
                    for domain in allow_domains {
                        if !policy.allow_domains.contains(domain) {
                            return Err(LeaseError::InvalidSpec {
                                reason: format!("fetch_url domain '{domain}' not in allow-list for tenant '{tenant}'"),
                            });
                        }
                    }
                }
            }
        }

        Ok(())
    }
}
