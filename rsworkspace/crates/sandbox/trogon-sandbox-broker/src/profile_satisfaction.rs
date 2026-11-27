use trogon_sandbox_contract::{CapabilityNeed, CapabilityRequirements, CapabilitySupport, ProviderCapabilityProfile};

/// Whether a provider capability profile can serve a set of capability requirements.
pub(crate) trait ProfileSatisfies {
    fn satisfies(&self, req: &CapabilityRequirements) -> bool;
}

fn met(need: CapabilityNeed, support: CapabilitySupport) -> bool {
    !need.is_required() || support.is_supported()
}

impl ProfileSatisfies for ProviderCapabilityProfile {
    fn satisfies(&self, req: &CapabilityRequirements) -> bool {
        let iso = &self.isolation;
        let life = &self.lifecycle;
        let checks = [
            met(req.needs_snapshot, self.snapshot.can_snapshot),
            met(req.needs_checkpoint_running, self.snapshot.can_checkpoint_running),
            met(req.needs_restore, self.snapshot.can_restore),
            req.min_isolation_tier
                .is_none_or(|min| iso.supported_tiers.iter().any(|t| *t >= min)),
            req.max_cold_restore_p50_ms
                .is_none_or(|max| self.performance.cold_restore_p50_ms <= max),
            met(req.needs_warm_pool, life.supports_warm_pool),
            met(req.needs_named_tags, life.supports_named_tags),
            met(req.needs_fuse_mounts, self.volume.supports_fuse_mounts),
            met(req.needs_privileged, iso.supports_privileged),
            met(req.needs_sandbox_users, iso.supports_sandbox_users),
            met(req.needs_interactive_stdin, iso.supports_interactive_stdin),
            met(req.needs_named_processes, iso.supports_named_processes),
            met(req.needs_output_streaming, iso.supports_output_streaming),
            met(req.needs_process_readiness, iso.supports_process_readiness),
            met(req.needs_graceful_stop, iso.supports_graceful_stop),
            met(req.needs_command_restart, iso.supports_command_restart),
            met(req.needs_port_preview, iso.supports_port_preview),
            req.exposed_port_count.is_none_or(|count| self.ingress.allows(count)),
            met(req.needs_domain_allowlist, iso.supports_domain_allowlist),
            !req.needs_cidr_allowlist.is_required()
                || iso.egress.cidr_allowlist.is_supported()
                || iso.supports_domain_allowlist.is_supported(),
            met(req.needs_hardening_overrides, iso.supports_hardening_overrides),
            req.min_egress_enforcement
                .is_none_or(|min| iso.egress_enforcement >= min),
            met(req.needs_tls_intercept, iso.supports_tls_intercept),
            met(req.needs_secret_substitution, iso.supports_secret_substitution),
            met(req.needs_secret_bindings, iso.supports_secret_bindings),
            met(req.needs_masked_files, iso.supports_masked_files),
            met(req.needs_fs_policy, iso.supports_fs_policy),
            met(req.needs_pause_resume, life.supports_pause_resume),
            req.needs_suspend
                .is_none_or(|level| life.dormancy.suspend == Some(level)),
            match (req.lease_lifetime_seconds, life.max_lease_lifetime_seconds) {
                (Some(need), Some(max)) => need <= max,
                _ => true,
            },
            met(req.needs_file_ops, iso.supports_file_ops),
            met(req.needs_file_search, iso.supports_file_search),
            met(req.needs_file_archive, iso.supports_file_archive),
            met(req.needs_output_redaction, iso.supports_output_redaction),
            met(req.needs_usage_gauges, self.resources.reports_usage_gauges),
            met(req.needs_terminal, iso.supports_terminal),
            met(req.needs_network_policy_update, iso.supports_network_policy_update),
            met(req.needs_sandbox_groups, iso.supports_sandbox_groups),
            req.vcpu_millis
                .is_none_or(|want| self.resources.vcpu_millis.is_some_and(|r| r.contains(want))),
            req.disk_mib
                .is_none_or(|want| self.resources.disk_mib.is_some_and(|r| r.contains(want))),
            met(req.needs_native_passthrough, self.placement.supports_native_passthrough),
            req.region
                .as_ref()
                .is_none_or(|region| self.placement.regions.contains(region)),
            req.env_digest
                .as_ref()
                .is_none_or(|want| self.placement.env_digest.as_ref() == Some(want)),
            met(req.needs_change_tracking, iso.supports_change_tracking),
            met(req.needs_file_watch, iso.supports_file_watch),
            met(req.needs_persistent_runtime, iso.supports_persistent_runtime),
            met(req.needs_dir_transfer, iso.supports_dir_transfer),
            met(req.needs_browser, iso.supports_browser),
        ];
        checks.into_iter().all(|ok| ok)
    }
}
