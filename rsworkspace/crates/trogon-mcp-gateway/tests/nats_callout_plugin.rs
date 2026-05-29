//! Integration tests for Tier 2.5 NATS-callout plugins (scaffold).
//!
//! Gateway wiring is not yet spliced; cases remain ignored until Block F item 7 lands in
//! `gateway.rs`.

#[cfg(test)]
mod gateway_wiring {
    #[tokio::test]
    #[ignore = "gateway pipeline not wired to PluginDispatcher yet"]
    async fn pre_authz_plugin_deny_drops_request_before_spicedb() {
        unimplemented!("wire PluginRegistry + PluginDispatcher into ingress authz path");
    }

    #[tokio::test]
    #[ignore = "gateway pipeline not wired to PluginDispatcher yet"]
    async fn pre_call_plugin_rewrite_updates_params_before_backend() {
        unimplemented!("apply PluginDecision::Rewrite params to egress payload");
    }

    #[tokio::test]
    #[ignore = "gateway pipeline not wired to PluginDispatcher yet"]
    async fn plugin_timeout_emits_plugin_failed_transient_audit() {
        unimplemented!("verify JetStream audit subject plugin_failed with transient detail");
    }
}
