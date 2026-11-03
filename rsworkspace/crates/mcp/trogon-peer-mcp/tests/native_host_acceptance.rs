//! Opt-in installed-host proof. Never run this as an ordinary CI model call.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use trogon_decider_runtime::{ReadFrom, ReadStreamRequest, StreamRead};
use trogon_peer::{EnrollmentIdentity, InstallationId, MessageId, NativeSessionId, PeerId, TrustedPeerOwner};
use trogon_peer_nats::peer::{PeerJetStreamStore, PeerStorageBudget, PeerStoreConfig};
use trogon_peer_nats::test_support::{
    application,
    broker::{Broker, TestResult, observed_barrier},
};
use trogon_peer_nats::{PeerAccount, PeerTenant, serve_peer_rpc};
use trogon_peer_service::{WorkLimit, test_support::Fixture};
use trogon_std::env::{EnumerateEnv, ReadEnv, SystemEnv};
use trogonai_proto::peers::state_v1alpha1::PeerState;
use trogonai_proto::peers::v1alpha1 as v1;

const TURN_DEADLINE: Duration = Duration::from_secs(60);
const TOOL_PREFIX: &str = "mcp__plugin_trogon-peer_peer__";

fn require(condition: bool, message: &'static str) -> TestResult {
    if condition { Ok(()) } else { Err(message.into()) }
}

struct Host {
    child: Child,
    input: Option<ChildStdin>,
    output: Lines<BufReader<ChildStdout>>,
    stderr: tokio::task::JoinHandle<std::io::Result<()>>,
    evidence: PathBuf,
    events: Vec<Value>,
    session: String,
    prompts: usize,
}

impl Host {
    fn start(broker: &Broker, identity: &str, session: &str, cwd: &Path, evidence: &Path) -> TestResult<Self> {
        let mut command = Command::new("claude");
        command
            .args([
                "--print",
                "--verbose",
                "--input-format",
                "stream-json",
                "--output-format",
                "stream-json",
                "--include-partial-messages",
                "--no-session-persistence",
                "--disable-slash-commands",
                "--max-budget-usd",
                "2",
                "--tools",
                "",
                "--setting-sources",
                "",
                "--session-id",
                session,
                "--plugin-dir",
            ])
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../../plugins/claude/peer"))
            .arg("--allowedTools")
            .arg(format!(
                "{TOOL_PREFIX}peer_roster,{TOOL_PREFIX}peer_send,{TOOL_PREFIX}peer_status,{TOOL_PREFIX}peer_acknowledge_batch"
            ))
            .arg("--disallowedTools")
            .arg(format!("{TOOL_PREFIX}peer_prompt_hook,{TOOL_PREFIX}peer_detach_hook"))
            .current_dir(cwd)
            .kill_on_drop(true)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (key, _) in SystemEnv.vars_os() {
            if key
                .to_str()
                .is_some_and(|key| key.starts_with("NATS_") || key.starts_with("PEER_"))
            {
                command.env_remove(key);
            }
        }
        let path = std::env::join_paths(
            std::iter::once(cwd.join("bin"))
                .chain(std::env::split_paths(&SystemEnv.var_os("PATH").ok_or("PATH absent")?)),
        )?;
        command
            .env("PATH", path)
            .env("NATS_URL", broker.server.address())
            .env("NATS_CA", broker.server.ca())
            .env(
                "NATS_NKEY",
                format!("file://{}", broker.credential_path(identity)?.display()),
            )
            .env("PEER_TENANT_ID", "tenant")
            .env("PEER_ACCOUNT_MODE", "shared_fixture")
            .env("PEER_CALLER_ID", identity)
            .env("PEER_CONNECTION_ID", "one")
            .env("PEER_INSTALLATION_ID", "install")
            .env("PEER_COLLABORATION_REF", "shared")
            .env("PEER_DISPLAY_NAME", identity)
            .env("PEER_AVAILABILITY", "ready");
        let mut child = command.spawn()?;
        let input = child.stdin.take().ok_or("host stdin absent")?;
        let output = BufReader::new(child.stdout.take().ok_or("host stdout absent")?).lines();
        let mut stderr = child.stderr.take().ok_or("host stderr absent")?.take(65_536);
        let error_path = evidence.with_extension("stderr.internal.trogonai.md");
        let stderr = tokio::spawn(async move {
            let mut file = tokio::fs::File::create(error_path).await?;
            tokio::io::copy(&mut stderr, &mut file).await?;
            Ok(())
        });
        Ok(Self {
            child,
            input: Some(input),
            output,
            stderr,
            evidence: evidence.to_owned(),
            events: Vec::new(),
            session: session.to_owned(),
            prompts: 0,
        })
    }

    async fn prompt(&mut self, text: &str) -> TestResult<Value> {
        self.prompts += 1;
        require(self.prompts <= 2, "per-host prompt cap exceeded")?;
        let frame = json!({"type":"user", "session_id":self.session,
            "message":{"role":"user", "content":text}, "parent_tool_use_id":null});
        let input = self.input.as_mut().ok_or("host stdin closed")?;
        input.write_all(format!("{frame}\n").as_bytes()).await?;
        input.flush().await?;
        tokio::time::timeout(TURN_DEADLINE, async {
            loop {
                let line = self.output.next_line().await?.ok_or("host exited before result")?;
                require(line.len() <= 1_048_576, "host event exceeds evidence bound")?;
                writeln!(
                    std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(&self.evidence)?,
                    "{line}"
                )?;
                let event: Value = serde_json::from_str(&line)?;
                let result = event["type"] == "result";
                self.events.push(event.clone());
                require(self.events.len() <= 4096, "host event count exceeds evidence bound")?;
                if event["type"] == "system" && event["subtype"] == "init" {
                    self.check_init(&event)?;
                }
                if result {
                    require(
                        event["subtype"] == "success",
                        "host did not complete prompt successfully",
                    )?;
                    require(
                        event["is_error"] != true,
                        "host returned an error; consult retained result event",
                    )?;
                    return Ok(event);
                }
            }
        })
        .await?
    }

    fn check_init(&self, event: &Value) -> TestResult {
        require(
            event["session_id"] == self.session,
            "host native session identity differs",
        )?;
        let servers = event["mcp_servers"].as_array().ok_or("host MCP inventory absent")?;
        require(servers.len() == 1, "unrelated MCP integrations are enabled")?;
        require(
            servers[0]["name"] == "plugin:trogon-peer:peer",
            "published peer plugin not loaded",
        )?;
        require(servers[0]["status"] == "connected", "peer MCP connection unavailable")?;
        let tools = event["tools"].as_array().ok_or("host tool inventory absent")?;
        require(tools.len() == 4, "unexpected host tool inventory")?;
        for tool in tools {
            require(
                ["peer_roster", "peer_send", "peer_status", "peer_acknowledge_batch"]
                    .iter()
                    .any(|name| tool == &format!("{TOOL_PREFIX}{name}")),
                "unrelated model tool is enabled",
            )?;
        }
        Ok(())
    }

    fn check_model_calls(&self) -> TestResult {
        require(
            self.calls("peer_prompt_hook").is_empty() && self.calls("peer_detach_hook").is_empty(),
            "model issued a manual hook call",
        )
    }

    fn calls(&self, name: &str) -> Vec<&Value> {
        self.events
            .iter()
            .filter(|event| event["type"] == "assistant")
            .filter_map(|event| event["message"]["content"].as_array())
            .flatten()
            .filter(|block| block["type"] == "tool_use" && block["name"] == format!("{TOOL_PREFIX}{name}"))
            .collect()
    }

    async fn finish(&mut self) -> TestResult {
        self.input.take();
        let status = tokio::time::timeout(Duration::from_secs(5), self.child.wait()).await??;
        self.collect_stderr().await?;
        self.check_model_calls()?;
        require(status.success(), "host exit was unsuccessful")?;
        Ok(())
    }

    async fn stop(&mut self) -> TestResult {
        self.input.take();
        if self.child.try_wait()?.is_none() {
            tokio::time::timeout(Duration::from_secs(5), self.child.kill()).await??;
        }
        self.collect_stderr().await?;
        Ok(())
    }

    async fn collect_stderr(&mut self) -> TestResult {
        match tokio::time::timeout(Duration::from_secs(5), &mut self.stderr).await {
            Ok(result) => result??,
            Err(error) => {
                self.stderr.abort();
                return Err(error.into());
            }
        }
        Ok(())
    }
}

async fn history(store: &PeerJetStreamStore, peer: &PeerId) -> TestResult<PeerState> {
    let read = store
        .read_stream_bounded(
            ReadStreamRequest {
                stream_id: peer,
                from: ReadFrom::Beginning,
            },
            4096,
        )
        .await?;
    let mut state = trogon_peer::commands::state::initial_state();
    for event in read.events {
        let event = event
            .decode::<v1::PeerEvent>()?
            .into_decoded()
            .ok_or("unknown peer event")?;
        state = trogon_peer::commands::state::evolve(state, &event)?;
    }
    Ok(state)
}

fn grant(fixture: &Fixture, owner: &TrustedPeerOwner, session: &str) -> TestResult<PeerId> {
    let identity = EnrollmentIdentity::new(
        owner.clone(),
        InstallationId::new("install")?,
        NativeSessionId::new(session)?,
    );
    let peer = identity.peer_id().clone();
    fixture
        .policy
        .grants
        .lock()
        .map_err(|_| "grant lock poisoned")?
        .insert(peer.to_string(), owner.clone());
    fixture
        .catalog
        .peers
        .lock()
        .map_err(|_| "catalog lock poisoned")?
        .push(peer.clone());
    Ok(peer)
}

#[tokio::test]
#[ignore = "explicit installed-host acceptance only; spends up to two dollars per host"]
async fn installed_host_next_prompt_context() -> TestResult {
    let evidence = PathBuf::from(
        SystemEnv
            .var_os("PEER_ACCEPTANCE_EVIDENCE")
            .ok_or("evidence directory required")?,
    );
    std::fs::create_dir(&evidence)?;
    let broker = Broker::start().await?;
    let (service, _) = broker.connect("service").await?;
    let fixture = Fixture::new().await?;
    fixture.policy.grants.lock().map_err(|_| "grant lock poisoned")?.clear();
    fixture
        .catalog
        .peers
        .lock()
        .map_err(|_| "catalog lock poisoned")?
        .clear();
    let sender_session = uuid::Uuid::new_v4().to_string();
    let recipient_session = uuid::Uuid::new_v4().to_string();
    let sender_id = grant(&fixture, &fixture.alice, &sender_session)?;
    let recipient_id = grant(&fixture, &fixture.bob, &recipient_session)?;
    let store = PeerJetStreamStore::open(
        async_nats::jetstream::new(service.clone()),
        &PeerStoreConfig::new(PeerTenant::new("tenant")?, PeerStorageBudget::new(16_777_216)?),
    )
    .await?;
    let backend = application::compose(store.clone(), &fixture);
    let registration = serve_peer_rpc(
        &service,
        PeerAccount::SharedFixture {
            tenant: PeerTenant::new("tenant")?,
        },
        backend.clone(),
    )
    .await?;
    observed_barrier(&service).await?;
    let cwd = tempfile::tempdir()?;
    std::fs::create_dir(cwd.path().join("bin"))?;
    std::fs::copy(
        env!("CARGO_BIN_EXE_trogon-peer-mcp"),
        cwd.path().join("bin/trogon-peer-mcp"),
    )?;
    let mut sender = Host::start(
        &broker,
        "alice",
        &sender_session,
        cwd.path(),
        &evidence.join("sender.internal.trogonai.md"),
    )?;
    let mut recipient = Host::start(
        &broker,
        "bob",
        &recipient_session,
        cwd.path(),
        &evidence.join("recipient.internal.trogonai.md"),
    )?;
    let result: TestResult = async {
        sender.prompt("Do not call any tools. Reply READY.").await?;
        require(history(&store, &sender_id).await?.enrollment.is_set(), "native UserPromptSubmit did not bind sender")?;
        recipient.prompt("Do not call any tools. If no peer context is attached, reply NO_PEER_CONTEXT.").await?;
        require(history(&store, &recipient_id).await?.enrollment.is_set(), "native UserPromptSubmit did not bind recipient")?;
        require(history(&store, &recipient_id).await?.inbox.is_empty(), "unexpected recipient inbox before send")?;
        let marker = format!("peer-proof-{}", uuid::Uuid::new_v4().simple());
        let send = format!("Call peer_roster to discover bob. Send bob the exact text {marker} using peer_send with idempotency_key acceptance_send. Repeat the identical send with the same key once to verify idempotency. Do not call any other tools. Then reply SENT.");
        sender.prompt(&send).await?;
        require(sender.calls("peer_roster").len() == 1, "sender did not discover recipient through peer_roster")?;
        let sends = sender.calls("peer_send");
        require(sends.len() == 2, "sender did not perform identical send retry")?;
        require(sends[0]["input"] == sends[1]["input"], "sender retried a different request")?;
        let state = history(&store, &sender_id).await?;
        require(state.outbox.len() == 1, "sender did not durably accept one idempotent message")?;
        let message = MessageId::new(state.outbox[0].message.message_id.clone().ok_or("message id absent")?)?;
        require(history(&store, &recipient_id).await?.inbox.is_empty(), "intake happened before trusted reconciliation")?;
        backend.application().reconcile(&fixture.alice, &sender_id, std::slice::from_ref(&message), WorkLimit::new(1)?).await?;
        require(history(&store, &recipient_id).await?.inbox.len() == 1, "trusted intake did not persist one message")?;
        let received = recipient.prompt("Report the exact fresh marker from peer context attached to this prompt. If there is none, reply NO_PEER_CONTEXT. If a batch identifier is supplied in that context, explicitly acknowledge that batch with peer_acknowledge_batch after reporting the marker. Use no other tools.").await?;
        require(received["result"].as_str().is_some_and(|text| text.contains(&marker)), "recipient did not observe marker from native context")?;
        let state = history(&store, &recipient_id).await?;
        require(state.batches.len() == 1, "native hook did not assign a batch")?;
        let acknowledged = state.batches[0].acknowledged_at.is_set();
        let before_settlement = history(&store, &sender_id).await?;
        backend.application().reconcile(&fixture.alice, &sender_id, &[message], WorkLimit::new(1)?).await?;
        let after_settlement = history(&store, &sender_id).await?;
        let receipt = json!({"marker_observed":true,"application_acknowledged":acknowledged,
            "sender_settlement_before_reconciliation":before_settlement.outbox[0].settlement,
            "sender_settlement_after_reconciliation":after_settlement.outbox[0].settlement,
            "sender_results":sender.events.iter().filter(|v|v["type"]=="result").collect::<Vec<_>>(),
            "recipient_results":recipient.events.iter().filter(|v|v["type"]=="result").collect::<Vec<_>>(),
            "clock":"static fixture clock; no lease timing claim", "production_sealing":false});
        std::fs::write(evidence.join("outcome.internal.trogonai.md"), serde_json::to_vec_pretty(&receipt)?)?;
        sender.check_model_calls()?;
        recipient.check_model_calls()?;
        Ok(())
    }.await;
    let backend_receipt = json!({
        "sender_state": history(&store, &sender_id).await.map_err(|error| error.to_string()),
        "recipient_state": history(&store, &recipient_id).await.map_err(|error| error.to_string()),
        "sender_submitted_prompts": sender.prompts,
        "recipient_submitted_prompts": recipient.prompts,
        "outcome": if result.is_ok() { "passed" } else { "incomplete" },
    });
    let sender_cleanup = if result.is_ok() {
        sender.finish().await
    } else {
        sender.stop().await
    };
    let recipient_cleanup = if result.is_ok() {
        recipient.finish().await
    } else {
        recipient.stop().await
    };
    let registration_cleanup = registration.stop().await;
    std::fs::write(
        evidence.join("backend.internal.trogonai.md"),
        serde_json::to_vec_pretty(&backend_receipt)?,
    )?;
    let cleanup = json!({
        "sender":sender_cleanup.as_ref().map_err(|error|error.to_string()),
        "recipient":recipient_cleanup.as_ref().map_err(|error|error.to_string()),
        "registration":registration_cleanup.as_ref().map_err(|error|error.to_string()),
    });
    std::fs::write(
        evidence.join("cleanup.internal.trogonai.md"),
        serde_json::to_vec_pretty(&cleanup)?,
    )?;
    result?;
    sender_cleanup?;
    recipient_cleanup?;
    registration_cleanup?;
    Ok(())
}
