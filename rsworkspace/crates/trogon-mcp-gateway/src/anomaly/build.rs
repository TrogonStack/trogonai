//! Derive per-request [`AnomalyFeatures`] from ingress identity context.

use crate::agent_identity::ActChainEntry;
use crate::egress::backend_target_aud;

use super::features::AnomalyFeatures;
use super::novelty::NoveltyTracker;
use super::rate::RateTracker;
use super::{now_unix, AnomalyError};

/// Identity inputs available after policy + SpiceDB authorization in ingress.
#[derive(Clone, Debug)]
pub struct AnomalyIngressContext<'a> {
    pub tenant_id: &'a str,
    pub agent_id: &'a str,
    pub purpose: &'a str,
    pub server_id: &'a str,
    pub act_chain: &'a [ActChainEntry],
    pub request_id: Option<&'a str>,
}

/// Owned ingress snapshot for async anomaly emission (after authorization).
#[derive(Clone, Debug)]
pub struct AnomalyIngressSnapshot {
    pub tenant_id: String,
    pub agent_id: String,
    pub purpose: String,
    pub server_id: String,
    pub act_chain_len: usize,
    pub request_id: Option<String>,
}

impl AnomalyIngressSnapshot {
    #[must_use]
    pub fn target_audience(&self) -> String {
        backend_target_aud(&self.tenant_id, &self.server_id)
    }

    pub fn build_features(
        &self,
        novelty: &mut NoveltyTracker,
        rate: &mut RateTracker,
    ) -> Result<AnomalyFeatures, AnomalyError> {
        let ts_unix_ms = now_unix().saturating_mul(1_000);
        let target = self.target_audience();
        let is_novel_triple = novelty.observe(
            &self.tenant_id,
            &self.agent_id,
            &self.purpose,
            &target,
            ts_unix_ms,
        );
        rate.record(&self.agent_id, ts_unix_ms);
        let exchange_rate_per_min = rate.exchange_rate_per_min(&self.agent_id, ts_unix_ms);
        let chain_depth = u32::try_from(self.act_chain_len).unwrap_or(u32::MAX);
        let request_id = self
            .request_id
            .clone()
            .unwrap_or_else(|| "unknown".to_string());

        Ok(AnomalyFeatures {
            chain_depth,
            is_novel_triple,
            exchange_rate_per_min,
            tenant_id: self.tenant_id.clone(),
            agent_id: self.agent_id.clone(),
            purpose: self.purpose.clone(),
            target,
            request_id,
            ts_unix_ms,
        })
    }
}

impl<'a> AnomalyIngressContext<'a> {
    #[must_use]
    pub fn to_snapshot(&self) -> AnomalyIngressSnapshot {
        AnomalyIngressSnapshot {
            tenant_id: self.tenant_id.to_string(),
            agent_id: self.agent_id.to_string(),
            purpose: self.purpose.to_string(),
            server_id: self.server_id.to_string(),
            act_chain_len: self.act_chain.len(),
            request_id: self.request_id.map(str::to_string),
        }
    }

    #[must_use]
    pub fn target_audience(&self) -> String {
        backend_target_aud(self.tenant_id, self.server_id)
    }

    pub fn build_features(
        &self,
        novelty: &mut NoveltyTracker,
        rate: &mut RateTracker,
    ) -> Result<AnomalyFeatures, AnomalyError> {
        let ts_unix_ms = now_unix().saturating_mul(1_000);
        let target = self.target_audience();
        let is_novel_triple =
            novelty.observe(self.tenant_id, self.agent_id, self.purpose, &target, ts_unix_ms);
        rate.record(self.agent_id, ts_unix_ms);
        let exchange_rate_per_min = rate.exchange_rate_per_min(self.agent_id, ts_unix_ms);
        let chain_depth = u32::try_from(self.act_chain.len()).unwrap_or(u32::MAX);
        let request_id = self
            .request_id
            .map(str::to_string)
            .unwrap_or_else(|| "unknown".to_string());

        Ok(AnomalyFeatures {
            chain_depth,
            is_novel_triple,
            exchange_rate_per_min,
            tenant_id: self.tenant_id.to_string(),
            agent_id: self.agent_id.to_string(),
            purpose: self.purpose.to_string(),
            target,
            request_id,
            ts_unix_ms,
        })
    }
}
