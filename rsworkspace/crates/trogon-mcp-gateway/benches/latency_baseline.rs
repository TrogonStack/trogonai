//! E2E added-latency baseline: direct `mcp-nats` vs `trogon-mcp-gateway` (ADR 0031).
//!
//! Spawns or connects to a live NATS broker, runs paired samples, and emits P50/P99 JSON artifacts.
//! Phase timings are derived from `tracing` span enter/exit (not wall-clock wrappers around phases).

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use clap::Parser;
use futures::StreamExt;
use mcp_nats::{Config as McpConfig, McpPrefix};
use serde::Serialize;
use serde_json::{Value, json};
use tokio::runtime::Runtime;
use tokio::sync::oneshot;
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Metadata, Subscriber};
use trogon_mcp_gateway::authz::AllowAllPermissionChecker;
use trogon_mcp_gateway::gateway::GatewaySettings;
use trogon_mcp_gateway::jwt::JwtValidator;
use trogon_mcp_gateway::policy::hierarchical::{
    MergeEngine, PolicyEffect, PolicyLevel, PolicyRule, PolicyStore, ScopeBundle, ScopeConfig, ScopeKey,
};
use trogon_mcp_gateway::redaction::{
    JsonPath, RedactionAction, RedactionDirection, RedactionRegistry, RedactionRule, RedactionRuleset,
};
use trogon_mcp_gateway::schema_cache::{
    CachedSchema, SchemaCache, SchemaCacheConfig, SchemaCacheRuntime, SchemaSource, ServerId,
    schema_cache_key_for_tool,
};
use trogon_nats::{NatsAuth, NatsConfig, connect};
use uuid::Uuid;

const SERVER_ID: &str = "fixture";
const TOOL_NAME: &str = "echo_tool";
const CONNECT_TIMEOUT_SECS: u64 = 15;
const GATEWAY_READY_MS: u64 = 350;

const PHASES: [&str; 5] = ["ingress", "authz", "cel", "redaction", "egress"];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BenchMethod {
    Initialize,
    ToolsList,
    ToolsCall,
    ResourcesRead,
}

impl BenchMethod {
    const ALL: [Self; 4] = [
        Self::Initialize,
        Self::ToolsList,
        Self::ToolsCall,
        Self::ResourcesRead,
    ];

    fn jsonrpc_method(self) -> &'static str {
        match self {
            Self::Initialize => "initialize",
            Self::ToolsList => "tools/list",
            Self::ToolsCall => "tools/call",
            Self::ResourcesRead => "resources/read",
        }
    }

    fn subject_lane(self) -> &'static str {
        match self {
            Self::Initialize => "initialize",
            Self::ToolsList => "tools.list",
            Self::ToolsCall => "tools.call",
            Self::ResourcesRead => "resources.read",
        }
    }

    fn adr_workload_hint(self) -> &'static str {
        match self {
            Self::ToolsCall => "shadow|redact",
            Self::ToolsList => "list-shape",
            _ => "n/a",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum GatewayMode {
    Passthrough,
    Full,
}

#[derive(Parser, Debug)]
#[command(
    name = "latency_baseline",
    about = "E2E added-latency baseline for trogon-mcp-gateway vs direct mcp-nats (ADR 0031)"
)]
struct Args {
    /// Recorded iterations per method and gateway mode (after warmup).
    #[arg(long, default_value_t = 1000)]
    iterations: usize,

    /// Discarded warmup iterations per method (paired direct+gateway samples).
    #[arg(long, default_value_t = 200)]
    warmup: usize,

    /// NATS broker URL. When omitted, starts in-process `nats-server` (must be on PATH).
    #[arg(long, env = "NATS_URL")]
    nats_url: Option<String>,
}

#[derive(Debug, Clone)]
struct SpanRecord {
    name: String,
    start: Instant,
}

struct PhaseCollector {
    next_id: AtomicU64,
    active: Mutex<HashMap<u64, SpanRecord>>,
    durations_ns: Mutex<HashMap<String, Vec<u64>>>,
}

impl Default for PhaseCollector {
    fn default() -> Self {
        Self {
            next_id: AtomicU64::new(1),
            active: Mutex::new(HashMap::new()),
            durations_ns: Mutex::new(HashMap::new()),
        }
    }
}

impl PhaseCollector {
    fn record(&self, name: &str, duration_ns: u64) {
        let phase = normalize_phase(name);
        if !PHASES.contains(&phase) {
            return;
        }
        self.durations_ns
            .lock()
            .expect("phase durations lock")
            .entry(phase.to_string())
            .or_default()
            .push(duration_ns);
    }

    fn snapshot_and_reset(&self) -> HashMap<String, Vec<u64>> {
        let mut guard = self.durations_ns.lock().expect("phase durations lock");
        let snap = std::mem::take(&mut *guard);
        snap
    }
}

struct BenchSubscriber {
    collector: Arc<PhaseCollector>,
}

impl Subscriber for BenchSubscriber {
    fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, attrs: &Attributes<'_>) -> Id {
        let id = self.collector.next_id.fetch_add(1, Ordering::Relaxed);
        self.collector.active.lock().expect("active spans lock").insert(
            id,
            SpanRecord {
                name: attrs.metadata().name().to_string(),
                start: Instant::now(),
            },
        );
        Id::from_u64(id)
    }

    fn record(&self, _span: &Id, _values: &Record<'_>) {}

    fn record_follows_from(&self, _span: &Id, _follows: &Id) {}

    fn event(&self, _event: &Event<'_>) {}

    fn enter(&self, _span: &Id) {}

    fn exit(&self, span: &Id) {
        let id = span.into_u64();
        let Some(record) = self.collector.active.lock().expect("active spans lock").remove(&id) else {
            return;
        };
        let elapsed = record.start.elapsed().as_nanos().min(u64::MAX as u128) as u64;
        self.collector.record(&record.name, elapsed);
    }
}

fn normalize_phase(span_name: &str) -> &str {
    if span_name == "ingress"
        || span_name == "authz"
        || span_name == "cel"
        || span_name == "redaction"
        || span_name == "egress"
    {
        return span_name;
    }
    if span_name.contains("handle_ingress") {
        return "ingress";
    }
    if span_name.contains("authz") {
        return "authz";
    }
    if span_name.contains("cel") || span_name.contains("policy") {
        return "cel";
    }
    if span_name.contains("redact") {
        return "redaction";
    }
    if span_name.contains("egress") {
        return "egress";
    }
    span_name
}

fn install_tracing(collector: Arc<PhaseCollector>) {
    let subscriber = BenchSubscriber { collector };
    let _ = tracing::subscriber::set_global_default(subscriber);
}

#[derive(Serialize)]
struct PercentileMs {
    p50: f64,
    p99: f64,
    samples: usize,
}

#[derive(Serialize)]
struct PhaseStats {
    p50: f64,
    p99: f64,
    samples: usize,
}

#[derive(Serialize)]
struct MethodModeResult {
    method: String,
    mode: String,
    direct_ms: PercentileMs,
    gateway_ms: PercentileMs,
    added_ms: PercentileMs,
    phases_ms: HashMap<String, PhaseStats>,
    payload_bytes: usize,
    adr_workload_hint: String,
}

#[derive(Serialize)]
struct BenchmarkArtifact {
    schema: String,
    git_sha: Option<String>,
    recorded_at: String,
    runner_profile: String,
    nats_url: String,
    iterations: usize,
    warmup: usize,
    results: Vec<MethodModeResult>,
    phases_note: String,
}

fn percentile_ms(mut samples_ns: Vec<u64>, p: f64) -> f64 {
    if samples_ns.is_empty() {
        return 0.0;
    }
    samples_ns.sort_unstable();
    let idx = ((samples_ns.len() as f64 - 1.0) * p / 100.0).round() as usize;
    samples_ns[idx] as f64 / 1_000_000.0
}

fn stats_from_samples(samples_ns: Vec<u64>) -> PercentileMs {
    let samples = samples_ns.len();
    PercentileMs {
        p50: percentile_ms(samples_ns.clone(), 50.0),
        p99: percentile_ms(samples_ns, 99.0),
        samples,
    }
}

fn phase_stats(samples: Vec<u64>) -> PhaseStats {
    let count = samples.len();
    PhaseStats {
        p50: percentile_ms(samples.clone(), 50.0),
        p99: percentile_ms(samples, 99.0),
        samples: count,
    }
}

fn utc_now_rfc3339() -> String {
    std::process::Command::new("date")
        .args(["-u", "+%Y-%m-%dT%H:%M:%SZ"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| format!("{}Z", d.as_secs()))
                .unwrap_or_else(|_| "unknown".into())
        })
}

fn git_sha() -> Option<String> {
    std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
}

fn nats_server_on_path() -> bool {
    std::process::Command::new("nats-server")
        .arg("-v")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success() || s.code() == Some(0))
        .unwrap_or(false)
}

struct NatsFixture {
    url: String,
    _child: Option<Child>,
}

impl NatsFixture {
    async fn start(cli_url: Option<String>) -> Result<Self, String> {
        if let Some(url) = cli_url {
            return Ok(Self { url, _child: None });
        }
        if !nats_server_on_path() {
            return Err(
                "nats-server not found on PATH and NATS_URL is unset. \
                 Install nats-server or set NATS_URL to an existing broker."
                    .into(),
            );
        }
        let port = 16000 + (std::process::id() as u16 % 20000);
        let url = format!("nats://127.0.0.1:{port}");
        let store_dir = std::env::temp_dir().join(format!("trogon-mcp-bench-js-{port}"));
        let config_path = std::env::temp_dir().join(format!("trogon-mcp-bench-{port}.conf"));
        fs::create_dir_all(&store_dir).map_err(|e| e.to_string())?;
        let store_path = store_dir.to_string_lossy();
        let config = format!(
            r#"
port: {port}
jetstream {{
  store_dir: "{store_path}"
}}
"#
        );
        fs::write(&config_path, config).map_err(|e| e.to_string())?;
        let child = Command::new("nats-server")
            .arg("-c")
            .arg(&config_path)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("failed to spawn nats-server: {e}"))?;
        for _ in 0..50 {
            if async_nats::connect(&url).await.is_ok() {
                return Ok(Self {
                    url,
                    _child: Some(child),
                });
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        Err("nats-server did not become reachable within 5s".into())
    }
}

fn request_payload(method: BenchMethod, request_id: u64) -> (Vec<u8>, usize) {
    let value = match method {
        BenchMethod::Initialize => json!({
            "jsonrpc": "2.0",
            "id": request_id,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": { "name": "latency-baseline", "version": "0.1.0" }
            }
        }),
        BenchMethod::ToolsList => json!({
            "jsonrpc": "2.0",
            "id": request_id,
            "method": "tools/list",
            "params": {}
        }),
        BenchMethod::ToolsCall => json!({
            "jsonrpc": "2.0",
            "id": request_id,
            "method": "tools/call",
            "params": {
                "name": TOOL_NAME,
                "arguments": {
                    "token": "sk-bench",
                    "note": "bench-note",
                    "secret": "bench-secret",
                    "meta": "bench-meta"
                }
            }
        }),
        BenchMethod::ResourcesRead => json!({
            "jsonrpc": "2.0",
            "id": request_id,
            "method": "resources/read",
            "params": { "uri": "file://bench/resource.txt" }
        }),
    };
    let bytes = serde_json::to_vec(&value).expect("static request json");
    let len = bytes.len();
    (bytes, len)
}

fn echo_response(method: BenchMethod, request_id: u64) -> Vec<u8> {
    let body = match method {
        BenchMethod::Initialize => json!({
            "jsonrpc": "2.0",
            "id": request_id,
            "result": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "serverInfo": { "name": "echo", "version": "0.1.0" }
            }
        }),
        BenchMethod::ToolsList => {
            let tools: Vec<Value> = (0..50)
                .map(|i| {
                    json!({
                        "name": format!("tool_{i}"),
                        "description": "bench",
                        "inputSchema": { "type": "object" }
                    })
                })
                .collect();
            json!({
                "jsonrpc": "2.0",
                "id": request_id,
                "result": { "tools": tools }
            })
        }
        BenchMethod::ToolsCall => json!({
            "jsonrpc": "2.0",
            "id": request_id,
            "result": {
                "token": "echo-token",
                "note": "echo-note",
                "secret": "echo-secret",
                "meta": "echo-meta"
            }
        }),
        BenchMethod::ResourcesRead => json!({
            "jsonrpc": "2.0",
            "id": request_id,
            "result": {
                "contents": [{
                    "uri": "file://bench/resource.txt",
                    "mimeType": "text/plain",
                    "text": "echo"
                }]
            }
        }),
    };
    serde_json::to_vec(&body).expect("static echo json")
}

fn lane_from_subject(subject: &str, prefix: &str) -> Option<BenchMethod> {
    let tail = subject.strip_prefix(&format!("{prefix}.server.{SERVER_ID}."))?;
    match tail {
        "initialize" => Some(BenchMethod::Initialize),
        "tools.list" => Some(BenchMethod::ToolsList),
        "tools.call" => Some(BenchMethod::ToolsCall),
        "resources.read" => Some(BenchMethod::ResourcesRead),
        _ => None,
    }
}

async fn spawn_echo_backend(
    client: Arc<async_nats::Client>,
    prefix: &str,
) -> Result<(), String> {
    let subject = format!("{prefix}.server.{SERVER_ID}.>");
    let mut subscription = client
        .subscribe(subject.clone())
        .await
        .map_err(|e| e.to_string())?;
    let backend = client.clone();
    let prefix_owned = prefix.to_string();
    tokio::spawn(async move {
        while let Some(msg) = subscription.next().await {
            let Some(reply) = msg.reply.clone() else {
                continue;
            };
            let Some(method) = lane_from_subject(msg.subject.as_str(), &prefix_owned) else {
                continue;
            };
            let request_id = jsonrpc_request_id(&msg.payload).unwrap_or(1);
            let body = echo_response(method, request_id);
            backend
                .publish_with_headers(reply.to_string(), async_nats::HeaderMap::new(), Bytes::from(body))
                .await
                .ok();
            backend.flush().await.ok();
        }
    });
    Ok(())
}

fn jsonrpc_request_id(payload: &[u8]) -> Option<u64> {
    let value: Value = serde_json::from_slice(payload).ok()?;
    match value.get("id") {
        Some(Value::Number(n)) => n.as_u64(),
        _ => None,
    }
}

fn direct_subject(prefix: &str, method: BenchMethod) -> String {
    format!("{prefix}.server.{SERVER_ID}.{}", method.subject_lane())
}

fn gateway_subject(prefix: &str, method: BenchMethod) -> String {
    format!("{prefix}.gateway.request.{SERVER_ID}.{}", method.subject_lane())
}

async fn measure_request(
    client: &async_nats::Client,
    subject: &str,
    payload: &[u8],
    phase_collector: &Arc<PhaseCollector>,
) -> Result<u64, String> {
    phase_collector.snapshot_and_reset();
    let _span = tracing::info_span!("latency_baseline.request", subject = subject).entered();
    let start = Instant::now();
    let response = client
        .request_with_headers(
            subject.to_string(),
            async_nats::HeaderMap::new(),
            Bytes::copy_from_slice(payload),
        )
        .await
        .map_err(|e| e.to_string())?;
    let elapsed = start.elapsed().as_nanos().min(u64::MAX as u128) as u64;
    let _: Value = serde_json::from_slice(&response.payload).map_err(|e| e.to_string())?;
    Ok(elapsed)
}

static FULL_POLICY: OnceLock<()> = OnceLock::new();

async fn install_full_policy_once() {
    if FULL_POLICY.get().is_some() {
        return;
    }

    // SAFETY: single-threaded bench setup before gateway tasks start.
    unsafe {
        std::env::set_var(
            trogon_mcp_gateway::policy::hierarchical::ENV_MCP_GATEWAY_HIERARCHICAL_POLICY_ENFORCE,
            "1",
        );
    }

    let store = Arc::new(PolicyStore::default());
    let engine = Arc::new(MergeEngine::new(store, true));
    engine.upsert_scope(
        ScopeKey {
            level: PolicyLevel::Org,
            tenant: None,
            server_group: None,
            server_id: None,
            method: None,
            tool: None,
        },
        ScopeBundle {
            rules: vec![PolicyRule {
                policy_id: "bench/org-allow".into(),
                revision: 1,
                effect: PolicyEffect::Allow,
                priority: 0,
                cel: "true".into(),
            }],
            config: ScopeConfig::default(),
        },
    );
    engine.upsert_scope(
        ScopeKey {
            level: PolicyLevel::Method,
            tenant: Some("unknown".into()),
            server_group: None,
            server_id: Some(SERVER_ID.into()),
            method: Some("tools/list".into()),
            tool: None,
        },
        ScopeBundle {
            rules: vec![PolicyRule {
                policy_id: "bench/list-filter".into(),
                revision: 1,
                effect: PolicyEffect::Allow,
                priority: 0,
                cel: "!mcp.tool.name.matches('^internal_.*')".into(),
            }],
            config: ScopeConfig::default(),
        },
    );
    let _ = trogon_mcp_gateway::policy::hierarchical::set_shared(engine);

    let registry = Arc::new(RedactionRegistry::new());
    let ruleset = RedactionRuleset::builder()
        .rule(RedactionRule {
            path: JsonPath::parse("$.params.token").expect("path"),
            action: RedactionAction::Hash,
        })
        .rule(RedactionRule {
            path: JsonPath::parse("$.params.note").expect("path"),
            action: RedactionAction::Mask,
        })
        .rule(RedactionRule {
            path: JsonPath::parse("$.result.secret").expect("path"),
            action: RedactionAction::Hash,
        })
        .rule(RedactionRule {
            path: JsonPath::parse("$.result.meta").expect("path"),
            action: RedactionAction::Drop,
        })
        .build();
    registry.register(SERVER_ID, TOOL_NAME, RedactionDirection::Request, ruleset.clone());
    registry.register(SERVER_ID, TOOL_NAME, RedactionDirection::Response, ruleset);
    let _ = RedactionRegistry::install(registry);

    let runtime = SchemaCacheRuntime::new(SchemaCacheConfig::default());
    let _ = SchemaCacheRuntime::install(Arc::clone(&runtime));
    let server_id = ServerId::new(SERVER_ID);
    let input_schema = json!({
        "type": "object",
        "properties": {
            "token": { "type": "string" },
            "note": { "type": "string" }
        }
    });
    let output_schema = json!({
        "type": "object",
        "properties": {
            "secret": { "type": "string" },
            "meta": { "type": "string" }
        }
    });
    let input_key = schema_cache_key_for_tool(&server_id, &input_schema);
    runtime
        .cache
        .put(
            input_key,
            CachedSchema {
                schema: input_schema,
                fetched_at: std::time::SystemTime::now(),
                source: SchemaSource::ExplicitRegistration,
            },
            TOOL_NAME,
        )
        .await
        .expect("seed input schema");
    let output_key = schema_cache_key_for_tool(&server_id, &output_schema);
    runtime
        .cache
        .put(
            output_key,
            CachedSchema {
                schema: output_schema,
                fetched_at: std::time::SystemTime::now(),
                source: SchemaSource::ExplicitRegistration,
            },
            &format!("{TOOL_NAME}:output"),
        )
        .await
        .expect("seed output schema");

    let _ = FULL_POLICY.set(());
}

async fn spawn_gateway(
    nats_conf: &NatsConfig,
    prefix: &McpPrefix,
    prefix_segment: &str,
    mode: GatewayMode,
    traces: trogon_mcp_gateway::trace::TraceStore,
) -> Result<(Arc<async_nats::Client>, oneshot::Sender<()>, tokio::task::JoinHandle<Result<(), trogon_mcp_gateway::GatewayError>>), String>
{
    let connect_timeout = std::time::Duration::from_secs(CONNECT_TIMEOUT_SECS);
    let mcp_conf = McpConfig::new(prefix.clone(), nats_conf.clone())
        .with_operation_timeout(std::time::Duration::from_secs(CONNECT_TIMEOUT_SECS));

    let (init_audit, queue_suffix) = match mode {
        GatewayMode::Passthrough => (false, "passthrough"),
        GatewayMode::Full => {
            install_full_policy_once().await;
            (true, "full")
        }
    };

    let checker: Arc<dyn trogon_mcp_gateway::authz::PermissionChecker> = Arc::new(AllowAllPermissionChecker);
    let settings = GatewaySettings {
        queue_group: format!("q-{prefix_segment}-{queue_suffix}"),
        audit_stream_name: format!("MCP_AUDIT_BENCH_{prefix_segment}"),
        init_audit_stream: init_audit,
        mcp: mcp_conf,
        jwt: JwtValidator::disabled().expect("jwt ingress off"),
        egress: None,
        chain_resolver: None,
        rate_limit: None,
        approval_gate: None,
        mesh_config: trogon_mcp_gateway::policy::MeshGatewayConfig::default(),
    };

    let gateway_client = Arc::new(connect(nats_conf, connect_timeout).await.map_err(|e| e.to_string())?);
    let run_client = gateway_client.clone();
    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    let join = tokio::spawn(async move {
        trogon_mcp_gateway::run(run_client, checker, traces, settings, async {
            shutdown_rx.await.ok();
        })
        .await
    });

    tokio::time::sleep(std::time::Duration::from_millis(GATEWAY_READY_MS)).await;
    Ok((gateway_client, shutdown_tx, join))
}

async fn shutdown_gateway(
    shutdown_tx: oneshot::Sender<()>,
    join: tokio::task::JoinHandle<Result<(), trogon_mcp_gateway::GatewayError>>,
) {
    shutdown_tx.send(()).ok();
    if let Ok(result) = join.await {
        result.ok();
    }
}

async fn run_mode_benchmark(
    client: Arc<async_nats::Client>,
    gateway_client: Arc<async_nats::Client>,
    prefix: &str,
    method: BenchMethod,
    mode_label: &str,
    iterations: usize,
    warmup: usize,
    phase_collector: Arc<PhaseCollector>,
) -> Result<MethodModeResult, String> {
    let (_, payload_bytes) = request_payload(method, 1);

    let mut direct_samples = Vec::with_capacity(iterations);
    let mut gateway_samples = Vec::with_capacity(iterations);
    let mut added_samples = Vec::with_capacity(iterations);
    let mut phase_accum: HashMap<String, Vec<u64>> = HashMap::new();

    let total = warmup + iterations;
    for i in 0..total {
        let (payload, _) = request_payload(method, i as u64 + 1);
        let direct_ns = measure_request(&client, &direct_subject(prefix, method), &payload, &phase_collector).await?;
        let gateway_ns = measure_request(
            &gateway_client,
            &gateway_subject(prefix, method),
            &payload,
            &phase_collector,
        )
        .await?;

        if i >= warmup {
            direct_samples.push(direct_ns);
            gateway_samples.push(gateway_ns);
            if gateway_ns >= direct_ns {
                added_samples.push(gateway_ns - direct_ns);
            } else {
                added_samples.push(0);
            }
            for (phase, samples) in phase_collector.snapshot_and_reset() {
                phase_accum.entry(phase).or_default().extend(samples);
            }
        }
    }

    let phases_ms = PHASES
        .iter()
        .map(|phase| {
            let samples = phase_accum.remove(*phase).unwrap_or_default();
            (phase.to_string(), phase_stats(samples))
        })
        .collect();

    Ok(MethodModeResult {
        method: method.jsonrpc_method().to_string(),
        mode: mode_label.to_string(),
        direct_ms: stats_from_samples(direct_samples),
        gateway_ms: stats_from_samples(gateway_samples),
        added_ms: stats_from_samples(added_samples),
        phases_ms,
        payload_bytes,
        adr_workload_hint: method.adr_workload_hint().to_string(),
    })
}

async fn run_direct_only_benchmark(
    client: Arc<async_nats::Client>,
    prefix: &str,
    method: BenchMethod,
    iterations: usize,
    warmup: usize,
    phase_collector: Arc<PhaseCollector>,
) -> Result<MethodModeResult, String> {
    let (_, payload_bytes) = request_payload(method, 1);
    let mut direct_samples = Vec::with_capacity(iterations);
    let total = warmup + iterations;

    for i in 0..total {
        let (payload, _) = request_payload(method, i as u64 + 1);
        let direct_ns =
            measure_request(&client, &direct_subject(prefix, method), &payload, &phase_collector).await?;
        if i >= warmup {
            direct_samples.push(direct_ns);
        }
        phase_collector.snapshot_and_reset();
    }

    let empty = stats_from_samples(vec![]);
    Ok(MethodModeResult {
        method: method.jsonrpc_method().to_string(),
        mode: "direct".to_string(),
        direct_ms: stats_from_samples(direct_samples),
        gateway_ms: stats_from_samples(vec![]),
        added_ms: empty,
        phases_ms: HashMap::new(),
        payload_bytes,
        adr_workload_hint: method.adr_workload_hint().to_string(),
    })
}

fn results_path() -> PathBuf {
    let ts = utc_now_rfc3339().replace(':', "-");
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("benches/results")
        .join(format!("{ts}.json"))
}

fn print_summary(results: &[MethodModeResult]) {
    eprintln!("\nlatency_baseline summary (p50/p99 ms):");
    eprintln!("{:<18} {:<22} {:>8} {:>8} {:>10}", "method", "mode", "p50", "p99", "metric");
    for row in results {
        if row.mode == "direct" {
            eprintln!(
                "{:<18} {:<22} {:>8.3} {:>8.3} {:>10}",
                row.method, row.mode, row.direct_ms.p50, row.direct_ms.p99, "direct"
            );
        } else {
            eprintln!(
                "{:<18} {:<22} {:>8.3} {:>8.3} {:>10}",
                row.method, row.mode, row.added_ms.p50, row.added_ms.p99, "added"
            );
        }
    }
}

async fn async_main(args: Args) -> Result<(), String> {
    let nats = NatsFixture::start(args.nats_url.clone()).await?;
    let connect_timeout = std::time::Duration::from_secs(CONNECT_TIMEOUT_SECS);
    let nats_conf = NatsConfig::new(vec![nats.url.clone()], NatsAuth::None);
    let client = Arc::new(
        connect(&nats_conf, connect_timeout)
            .await
            .map_err(|e| e.to_string())?,
    );

    let prefix_segment = format!("{}", Uuid::now_v7().as_simple());
    let prefix_token = format!("{prefix_segment}.mcp");
    let prefix = McpPrefix::new(&prefix_token).map_err(|e| e.to_string())?;
    let prefix_str = prefix.as_str().to_string();

    spawn_echo_backend(client.clone(), &prefix_str).await?;
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let phase_collector = Arc::new(PhaseCollector::default());
    install_tracing(Arc::clone(&phase_collector));

    let mut results = Vec::new();

    for method in BenchMethod::ALL {
        results.push(
            run_direct_only_benchmark(
                client.clone(),
                &prefix_str,
                method,
                args.iterations,
                args.warmup,
                Arc::clone(&phase_collector),
            )
            .await?,
        );
    }

    for mode in [GatewayMode::Passthrough, GatewayMode::Full] {
        let mode_label = match mode {
            GatewayMode::Passthrough => "gateway-passthrough",
            GatewayMode::Full => "gateway-full",
        };
        let traces = trogon_mcp_gateway::trace::TraceStore::default();
        let (gateway_client, shutdown_tx, join) =
            spawn_gateway(&nats_conf, &prefix, &prefix_segment, mode, traces).await?;
        for method in BenchMethod::ALL {
            results.push(
                run_mode_benchmark(
                    client.clone(),
                    gateway_client.clone(),
                    &prefix_str,
                    method,
                    mode_label,
                    args.iterations,
                    args.warmup,
                    Arc::clone(&phase_collector),
                )
                .await?,
            );
        }
        shutdown_gateway(shutdown_tx, join).await;
    }

    let artifact = BenchmarkArtifact {
        schema: "trogon.mcp.benchmark.gateway/v1".into(),
        git_sha: git_sha(),
        recorded_at: utc_now_rfc3339(),
        runner_profile: format!("{}-nats-local", std::env::consts::OS),
        nats_url: nats.url,
        iterations: args.iterations,
        warmup: args.warmup,
        results,
        phases_note: "ingress mapped from mcp_gateway.handle_ingress; authz/cel/redaction/egress await gateway span instrumentation".into(),
    };

    print_summary(&artifact.results);
    let json = serde_json::to_string_pretty(&artifact).map_err(|e| e.to_string())?;
    println!("{json}");
    let path = results_path();
    fs::write(&path, &json).map_err(|e| format!("write {}: {e}", path.display()))?;
    eprintln!("\nWrote {}", path.display());
    Ok(())
}

fn parse_args() -> Args {
    let filtered: Vec<String> = std::env::args()
        .filter(|arg| arg != "--bench")
        .collect();
    Args::parse_from(filtered)
}

fn main() {
    let args = parse_args();
    let runtime = Runtime::new().expect("tokio runtime");
    if let Err(err) = runtime.block_on(async_main(args)) {
        eprintln!("latency_baseline failed: {err}");
        std::process::exit(1);
    }
}
