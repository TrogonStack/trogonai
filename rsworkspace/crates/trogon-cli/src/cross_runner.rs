use std::collections::HashMap;

use acp_nats::{AcpPrefix, Bridge, Config, NatsJetStreamClient};
use agent_client_protocol::{Agent as _, CloseSessionRequest, ExtRequest, NewSessionRequest};
use trogon_registry::{Registry, RegistryStore};
use trogon_std::time::SystemClock;

type ConcreteBridge = Bridge<async_nats::Client, SystemClock, NatsJetStreamClient>;

// ── RunnerSwitcher trait ──────────────────────────────────────────────────────

pub trait RunnerSwitcher {
    fn switch_model<'a>(
        &'a mut self,
        current_prefix: &'a str,
        current_session_id: &'a str,
        model_id: &'a str,
        cwd: &'a str,
    ) -> impl std::future::Future<Output = Result<(String, String), String>> + 'a;
}

// ── CrossRunnerSwitcher ───────────────────────────────────────────────────────

pub struct CrossRunnerSwitcher<S: RegistryStore> {
    nats: async_nats::Client,
    base_config: Config,
    registry: Registry<S>,
    bridges: HashMap<String, ConcreteBridge>,
}

impl<S: RegistryStore> CrossRunnerSwitcher<S> {
    pub fn new(nats: async_nats::Client, base_config: Config, registry: Registry<S>) -> Self {
        Self { nats, base_config, registry, bridges: HashMap::new() }
    }

    /// Switch the active session to whichever runner owns `model_id`.
    /// Returns `(target_prefix, new_session_id)`.
    /// Returns the original pair unchanged if the model is already on the current runner.
    ///
    /// Must be called from within a `tokio::task::LocalSet` — Bridge is `!Send`.
    pub async fn switch_model(
        &mut self,
        current_prefix: &str,
        current_session_id: &str,
        model_id: &str,
        cwd: &str,
    ) -> Result<(String, String), String> {
        // 1. Resolve target runner from registry
        let cap = self.registry
            .find_by_model(model_id).await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("no runner found for model: {model_id}"))?;
        let target_prefix = cap.metadata["acp_prefix"]
            .as_str()
            .ok_or("missing acp_prefix in registry metadata")?
            .to_string();
        if target_prefix == current_prefix {
            return Ok((current_prefix.to_string(), current_session_id.to_string()));
        }

        // 2. Ensure both bridges exist before any borrow
        self.ensure_bridge(current_prefix)?;
        self.ensure_bridge(&target_prefix)?;

        // 3. Export history as raw JSON
        let export_params = serde_json::value::RawValue::from_string(
            serde_json::json!({ "sessionId": current_session_id }).to_string(),
        )
        .map_err(|e| e.to_string())?;
        let messages_json = {
            let bridge = self.bridges.get(current_prefix).unwrap();
            bridge
                .ext_method(ExtRequest::new("session/export", export_params.into()))
                .await
                .map_err(|e| e.to_string())?
                .0
        };

        // 4. Open new session on target runner (same workspace path)
        let new_session_id = {
            let bridge = self.bridges.get(&target_prefix).unwrap();
            bridge
                .new_session(NewSessionRequest::new(cwd))
                .await
                .map_err(|e| e.to_string())?
                .session_id
                .to_string()
        };

        // 5. Import: pipe export JSON directly without re-serialization.
        // CrossRunnerSwitcher is agnostic to the message format — no deserialization needed.

        // LOW-6: Guard against null export — {"messages":null} would corrupt the target session.
        let raw_messages = messages_json.get();
        if raw_messages.trim() == "null" || raw_messages.trim().is_empty() {
            return Err("session/export returned null — cannot import into new session".into());
        }

        let import_params = serde_json::value::RawValue::from_string(format!(
            r#"{{"sessionId":"{new_session_id}","messages":{raw_messages}}}"#
        ))
        .map_err(|e| e.to_string())?;
        {
            let bridge = self.bridges.get(&target_prefix).unwrap();
            if let Err(import_err) = bridge
                .ext_method(ExtRequest::new("session/import", import_params.into()))
                .await
            {
                // MED-26: the target runner already opened new_session_id. If import
                // fails we'd otherwise leak that empty session until LRU eviction —
                // best-effort close it before surfacing the original error.
                let _ = bridge
                    .close_session(CloseSessionRequest::new(new_session_id.as_str()))
                    .await;
                return Err(import_err.to_string());
            }
        }

        Ok((target_prefix, new_session_id))
    }

    fn ensure_bridge(&mut self, prefix: &str) -> Result<(), String> {
        if self.bridges.contains_key(prefix) {
            return Ok(());
        }
        let acp_prefix = AcpPrefix::new(prefix).map_err(|e| e.to_string())?;
        let config = self.base_config.for_prefix(acp_prefix);
        let js = NatsJetStreamClient::new(async_nats::jetstream::new(self.nats.clone()));
        // LOW-7: The notification receiver is intentionally dropped here. The Bridge only sends
        // notifications during an active prompt stream; new_session and ext_method never
        // trigger notifications, so dropping the receiver is safe for this use-case.
        let (notification_tx, _notification_rx_intentionally_dropped) = tokio::sync::mpsc::channel(1);
        let bridge = Bridge::new(
            self.nats.clone(),
            js,
            SystemClock,
            &opentelemetry::global::meter("trogon-cli"),
            config,
            notification_tx,
        );
        self.bridges.insert(prefix.to_string(), bridge);
        Ok(())
    }
}

impl<S: RegistryStore> RunnerSwitcher for CrossRunnerSwitcher<S> {
    fn switch_model<'a>(
        &'a mut self,
        current_prefix: &'a str,
        current_session_id: &'a str,
        model_id: &'a str,
        cwd: &'a str,
    ) -> impl std::future::Future<Output = Result<(String, String), String>> + 'a {
        CrossRunnerSwitcher::switch_model(self, current_prefix, current_session_id, model_id, cwd)
    }
}

// ── MockRunnerSwitcher (test support) ─────────────────────────────────────────

#[cfg(test)]
pub mod mock {
    use super::RunnerSwitcher;

    pub struct MockRunnerSwitcher {
        result: Result<(String, String), String>,
    }

    impl MockRunnerSwitcher {
        pub fn same_runner(prefix: &str, session_id: &str) -> Self {
            Self { result: Ok((prefix.to_string(), session_id.to_string())) }
        }
        pub fn cross_runner(new_prefix: &str, new_session_id: &str) -> Self {
            Self { result: Ok((new_prefix.to_string(), new_session_id.to_string())) }
        }
        pub fn error(msg: &str) -> Self {
            Self { result: Err(msg.to_string()) }
        }
    }

    impl RunnerSwitcher for MockRunnerSwitcher {
        fn switch_model<'a>(
            &'a mut self,
            _current_prefix: &'a str,
            _current_session_id: &'a str,
            _model_id: &'a str,
            _cwd: &'a str,
        ) -> impl std::future::Future<Output = Result<(String, String), String>> + 'a {
            let result = self.result.clone();
            async move { result }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use acp_nats::AcpPrefix;
    use testcontainers_modules::nats::Nats;
    use testcontainers_modules::testcontainers::runners::AsyncRunner;
    use trogon_nats::{NatsAuth, NatsConfig};
    use trogon_registry::{AgentCapability, MockRegistryStore, Registry};

    async fn start_nats() -> (testcontainers_modules::testcontainers::ContainerAsync<Nats>, u16) {
        let container = Nats::default()
            .start()
            .await
            .expect("NATS container failed — is Docker running?");
        let port = container.get_host_port_ipv4(4222).await.unwrap();
        (container, port)
    }

    fn make_config(port: u16) -> Config {
        let nats_cfg = NatsConfig {
            servers: vec![format!("127.0.0.1:{port}")],
            auth: NatsAuth::None,
        };
        Config::new(AcpPrefix::new("acp").unwrap(), nats_cfg)
    }

    async fn connect(port: u16) -> async_nats::Client {
        async_nats::connect(format!("127.0.0.1:{port}"))
            .await
            .expect("failed to connect to NATS")
    }

    fn cap_with_prefix(model: &str, acp_prefix: &str) -> AgentCapability {
        let mut cap = AgentCapability::new("runner", ["chat"], "agents.runner.>");
        cap.metadata = serde_json::json!({ "models": [model], "acp_prefix": acp_prefix });
        cap
    }

    #[tokio::test]
    async fn same_runner_returns_unchanged() {
        let (_container, port) = start_nats().await;
        let registry = Registry::new(MockRegistryStore::new());
        registry.register(&cap_with_prefix("gpt-4", "acp.test")).await.unwrap();

        let mut switcher = CrossRunnerSwitcher::new(connect(port).await, make_config(port), registry);
        let result = switcher.switch_model("acp.test", "session-abc", "gpt-4", "/workspace").await;
        assert_eq!(result, Ok(("acp.test".to_string(), "session-abc".to_string())));
    }

    #[tokio::test]
    async fn model_not_found_returns_error() {
        let (_container, port) = start_nats().await;
        let registry = Registry::new(MockRegistryStore::new());

        let mut switcher = CrossRunnerSwitcher::new(connect(port).await, make_config(port), registry);
        let result = switcher.switch_model("acp.current", "session-1", "unknown-model", "/ws").await;
        assert_eq!(result, Err("no runner found for model: unknown-model".to_string()));
    }

    #[tokio::test]
    async fn missing_acp_prefix_returns_error() {
        let (_container, port) = start_nats().await;
        let registry = Registry::new(MockRegistryStore::new());
        let mut cap = AgentCapability::new("runner", ["chat"], "agents.runner.>");
        cap.metadata = serde_json::json!({ "models": ["gpt-4"] });
        registry.register(&cap).await.unwrap();

        let mut switcher = CrossRunnerSwitcher::new(connect(port).await, make_config(port), registry);
        let result = switcher.switch_model("acp.other", "session-1", "gpt-4", "/ws").await;
        assert_eq!(result, Err("missing acp_prefix in registry metadata".to_string()));
    }

    #[tokio::test]
    async fn invalid_target_prefix_returns_error() {
        let (_container, port) = start_nats().await;
        let registry = Registry::new(MockRegistryStore::new());
        // "acp..bad" has consecutive dots — AcpPrefix::new rejects it
        registry.register(&cap_with_prefix("gpt-4", "acp..bad")).await.unwrap();

        let mut switcher = CrossRunnerSwitcher::new(connect(port).await, make_config(port), registry);
        let result = switcher.switch_model("acp.current", "session-1", "gpt-4", "/ws").await;
        assert!(result.is_err(), "expected Err but got: {result:?}");
    }

    // ── Happy path: full cross-runner migration ───────────────────────────────

    /// Sets up a NATS subscriber that responds once to `subject` with `response_bytes`.
    async fn mock_responder(nats: async_nats::Client, subject: &'static str, response_bytes: &'static [u8]) {
        use futures::StreamExt as _;
        let mut sub = nats.subscribe(subject).await.expect("subscribe");
        tokio::spawn(async move {
            if let Some(msg) = sub.next().await {
                if let Some(reply) = msg.reply {
                    nats.publish(reply, axum::body::Bytes::from_static(response_bytes))
                        .await
                        .ok();
                }
            }
        });
    }

    #[tokio::test]
    async fn cross_runner_migration_succeeds() {
        let (_container, port) = start_nats().await;
        let nats_bg = async_nats::connect(format!("127.0.0.1:{port}")).await.unwrap();

        // session/export → any valid JSON (ExtResponse is transparent)
        mock_responder(nats_bg.clone(), "acp.src.agent.ext.session/export", b"[]").await;
        // new_session → {"sessionId":"migrated-session"}
        mock_responder(
            nats_bg.clone(),
            "acp.tgt.agent.session.new",
            br#"{"sessionId":"migrated-session"}"#,
        )
        .await;
        // session/import → any valid JSON
        mock_responder(nats_bg.clone(), "acp.tgt.agent.ext.session/import", b"{}").await;

        // small delay so all subscriptions are registered before the bridge sends requests
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        let registry = Registry::new(MockRegistryStore::new());
        registry.register(&cap_with_prefix("gpt-4", "acp.tgt")).await.unwrap();

        let mut switcher = CrossRunnerSwitcher::new(connect(port).await, make_config(port), registry);
        let result = switcher
            .switch_model("acp.src", "old-session-id", "gpt-4", "/workspace")
            .await;

        assert_eq!(result, Ok(("acp.tgt".to_string(), "migrated-session".to_string())));
    }

    // ── Error propagation ─────────────────────────────────────────────────────

    /// Like `mock_responder` but handles `count` consecutive messages on the same subject.
    async fn multi_responder(nats: async_nats::Client, subject: &str, responses: Vec<&'static [u8]>) {
        use futures::StreamExt as _;
        let mut sub = nats.subscribe(subject.to_string()).await.expect("subscribe");
        tokio::spawn(async move {
            for response in responses {
                if let Some(msg) = sub.next().await {
                    if let Some(reply) = msg.reply {
                        nats.publish(reply, axum::body::Bytes::from_static(response)).await.ok();
                    }
                }
            }
        });
    }

    /// Subscribes to `subject`, captures the first request payload, and replies with `response_bytes`.
    async fn capturing_responder(
        nats: async_nats::Client,
        subject: &str,
        response_bytes: &'static [u8],
    ) -> tokio::sync::oneshot::Receiver<Vec<u8>> {
        use futures::StreamExt as _;
        let (tx, rx) = tokio::sync::oneshot::channel();
        let mut sub = nats.subscribe(subject.to_string()).await.expect("subscribe");
        tokio::spawn(async move {
            if let Some(msg) = sub.next().await {
                let payload = msg.payload.to_vec();
                if let Some(reply) = msg.reply {
                    nats.publish(reply, axum::body::Bytes::from_static(response_bytes)).await.ok();
                }
                tx.send(payload).ok();
            }
        });
        rx
    }

    #[tokio::test]
    async fn export_failure_propagates_error() {
        let (_container, port) = start_nats().await;
        // No responder for session/export → NATS returns "no responders" immediately
        let registry = Registry::new(MockRegistryStore::new());
        registry.register(&cap_with_prefix("gpt-4", "acp.tgt")).await.unwrap();

        let mut switcher = CrossRunnerSwitcher::new(connect(port).await, make_config(port), registry);
        let result = switcher.switch_model("acp.src", "session-1", "gpt-4", "/ws").await;
        assert!(result.is_err(), "expected export failure to propagate; got: {result:?}");
    }

    #[tokio::test]
    async fn new_session_failure_propagates_error() {
        let (_container, port) = start_nats().await;
        let nats_bg = async_nats::connect(format!("127.0.0.1:{port}")).await.unwrap();

        mock_responder(nats_bg.clone(), "acp.src.agent.ext.session/export", b"[]").await;
        // No responder for acp.tgt.agent.session.new → error
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        let registry = Registry::new(MockRegistryStore::new());
        registry.register(&cap_with_prefix("gpt-4", "acp.tgt")).await.unwrap();

        let mut switcher = CrossRunnerSwitcher::new(connect(port).await, make_config(port), registry);
        let result = switcher.switch_model("acp.src", "session-1", "gpt-4", "/ws").await;
        assert!(result.is_err(), "expected new_session failure to propagate; got: {result:?}");
    }

    #[tokio::test]
    async fn import_failure_propagates_error() {
        let (_container, port) = start_nats().await;
        let nats_bg = async_nats::connect(format!("127.0.0.1:{port}")).await.unwrap();

        mock_responder(nats_bg.clone(), "acp.src.agent.ext.session/export", b"[]").await;
        mock_responder(nats_bg.clone(), "acp.tgt.agent.session.new", br#"{"sessionId":"s1"}"#).await;
        // No responder for session/import → error
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        let registry = Registry::new(MockRegistryStore::new());
        registry.register(&cap_with_prefix("gpt-4", "acp.tgt")).await.unwrap();

        let mut switcher = CrossRunnerSwitcher::new(connect(port).await, make_config(port), registry);
        let result = switcher.switch_model("acp.src", "session-1", "gpt-4", "/ws").await;
        assert!(result.is_err(), "expected import failure to propagate; got: {result:?}");
    }

    // ── Bridge reuse ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn bridge_is_reused_across_switch_model_calls() {
        let (_container, port) = start_nats().await;
        let nats_bg = async_nats::connect(format!("127.0.0.1:{port}")).await.unwrap();

        // Each subject handles 2 consecutive messages (one per switch_model call).
        // Using multi_responder avoids the race where two separate subscribers both
        // receive the first request and leave the second call without a responder.
        multi_responder(nats_bg.clone(), "acp.src.agent.ext.session/export", vec![b"[]", b"[]"]).await;
        multi_responder(
            nats_bg.clone(),
            "acp.tgt.agent.session.new",
            vec![br#"{"sessionId":"s1"}"#, br#"{"sessionId":"s2"}"#],
        )
        .await;
        multi_responder(nats_bg.clone(), "acp.tgt.agent.ext.session/import", vec![b"{}", b"{}"]).await;

        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        let registry = Registry::new(MockRegistryStore::new());
        registry.register(&cap_with_prefix("gpt-4", "acp.tgt")).await.unwrap();

        let mut switcher = CrossRunnerSwitcher::new(connect(port).await, make_config(port), registry);

        let r1 = switcher.switch_model("acp.src", "old-1", "gpt-4", "/ws").await;
        assert_eq!(r1, Ok(("acp.tgt".to_string(), "s1".to_string())));

        let r2 = switcher.switch_model("acp.src", "old-2", "gpt-4", "/ws").await;
        assert_eq!(r2, Ok(("acp.tgt".to_string(), "s2".to_string())));
    }

    // ── Import payload correctness ────────────────────────────────────────────

    #[tokio::test]
    async fn import_payload_contains_exported_messages() {
        let (_container, port) = start_nats().await;
        let nats_bg = async_nats::connect(format!("127.0.0.1:{port}")).await.unwrap();

        // Export returns a concrete messages array
        mock_responder(
            nats_bg.clone(),
            "acp.src.agent.ext.session/export",
            br#"[{"role":"user","content":"hello"}]"#,
        )
        .await;
        mock_responder(nats_bg.clone(), "acp.tgt.agent.session.new", br#"{"sessionId":"new-sess"}"#).await;
        // Capture the import request so we can inspect its body
        let import_rx =
            capturing_responder(nats_bg.clone(), "acp.tgt.agent.ext.session/import", b"{}").await;

        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        let registry = Registry::new(MockRegistryStore::new());
        registry.register(&cap_with_prefix("gpt-4", "acp.tgt")).await.unwrap();

        let mut switcher = CrossRunnerSwitcher::new(connect(port).await, make_config(port), registry);
        switcher.switch_model("acp.src", "src-session", "gpt-4", "/ws").await.unwrap();

        let import_body = import_rx.await.expect("import responder captured payload");
        let json: serde_json::Value = serde_json::from_slice(&import_body).unwrap();
        assert_eq!(json["sessionId"], "new-sess");
        assert_eq!(json["messages"], serde_json::json!([{"role": "user", "content": "hello"}]));
    }

    #[tokio::test]
    async fn import_payload_passes_v2_export_unchanged() {
        let (_container, port) = start_nats().await;
        let nats_bg = async_nats::connect(format!("127.0.0.1:{port}")).await.unwrap();

        let v2_export = br#"{"version":2,"messages":[{"version":2,"role":"user","blocks":[{"type":"text","text":"hi"}]}]}"#;
        mock_responder(nats_bg.clone(), "acp.src.agent.ext.session/export", v2_export).await;
        mock_responder(nats_bg.clone(), "acp.tgt.agent.session.new", br#"{"sessionId":"v2-sess"}"#).await;
        let import_rx =
            capturing_responder(nats_bg.clone(), "acp.tgt.agent.ext.session/import", b"{}").await;

        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        let registry = Registry::new(MockRegistryStore::new());
        registry.register(&cap_with_prefix("gpt-4", "acp.tgt")).await.unwrap();

        let mut switcher = CrossRunnerSwitcher::new(connect(port).await, make_config(port), registry);
        switcher.switch_model("acp.src", "src-session", "gpt-4", "/ws").await.unwrap();

        let import_body = import_rx.await.expect("import responder captured payload");
        let json: serde_json::Value = serde_json::from_slice(&import_body).unwrap();
        assert_eq!(json["sessionId"], "v2-sess");
        assert_eq!(json["messages"]["version"], 2);
        // Verify the text block is parseable by the import handler
        let block = &json["messages"]["messages"][0]["blocks"][0];
        assert_eq!(block["type"], "text");
        assert_eq!(block["text"], "hi");
    }
}
