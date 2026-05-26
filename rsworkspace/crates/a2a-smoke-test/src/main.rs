use std::path::PathBuf;
use std::time::Duration;

use a2a_auth_callout::{
    jwt::ExternalSubject, AccountName, CallerId, MintedUserJwt, SpiceDbPrincipal, UserJwtClaims, UserJwtSubject,
};
use a2a_auth_callout::permissions::IssuedPermissions;
use a2a_auth_callout::signing_key_source::{KeyVersion, SigningKeySource, StaticSigningKeySource};
use a2a_nats::client::unary::send_unary;
use a2a_nats::client::{Client, ClientError};
use a2a_nats::{
    compose_gateway_ingress_subject, A2aAgentId, A2aPrefix, Config, NatsConfig, ReqId,
};
use a2a_nats_discovery::{
    parse_operator_keys, resolve_operator_signature_gate, sign_discovery_export, OperatorKeyId,
    OperatorSignatureGate,
};
use a2a_types::{GetTaskRequest, Message, Part, Role, SendMessageRequest, TaskState};
use clap::{Parser, Subcommand, ValueEnum};
use ed25519_dalek::SigningKey;
use futures::StreamExt;
use serde::Deserialize;
use tracing::{error, info};

const TIER3_REFUSE_TRIGGER: &str = "SMOKE_T3_REFUSE_ME";

#[derive(Clone, Copy, Debug, ValueEnum)]
enum SmokeProfile {
    Smoke,
    Full,
}

#[derive(Parser)]
#[command(name = "a2a-smoke-test")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Mint a long-lived caller JWT for compose bootstrap (writes one line to --out).
    MintJwt {
        #[arg(long, env = "AUTH_CALLOUT_SIGNING_KEY_PATH")]
        signing_key_path: PathBuf,
        #[arg(long, default_value = "smoke-caller-1")]
        caller_id: String,
        #[arg(long, default_value = "APP")]
        audience: String,
        #[arg(long, default_value = "86400")]
        ttl_secs: u64,
        #[arg(long)]
        out: PathBuf,
        #[arg(long)]
        creds_out: Option<PathBuf>,
    },
    /// Hex-encode the ed25519 public key for a 32-byte seed (no 0x prefix).
    Ed25519Pubkey {
        #[arg(long)]
        seed_hex: String,
    },
    /// End-to-end smoke against compose-network NATS.
    Run {
        #[arg(long, value_enum, default_value_t = SmokeProfile::Smoke)]
        profile: SmokeProfile,
        #[arg(long, env = "NATS_URL", default_value = "nats://127.0.0.1:4222")]
        nats_url: String,
        #[arg(long, env = "A2A_GATEWAY_CALLER_JWT")]
        caller_jwt: String,
        #[arg(long, env = "A2A_AGENT_ID", default_value = "echo")]
        agent_id: String,
        #[arg(long, env = "A2A_PREFIX", default_value = "a2a")]
        prefix: String,
        #[arg(long, default_value = "30")]
        timeout_secs: u64,
    },
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct AuditEnvelopeWire {
    #[serde(default)]
    caller_id: Option<String>,
    #[serde(default)]
    caller_source: Option<String>,
    #[serde(default)]
    rules_fired: Option<Vec<String>>,
    #[serde(default)]
    refusal_skill: Option<String>,
    method: String,
    outcome: serde_json::Value,
}

type SmokeClient = Client<async_nats::Client, trogon_nats::jetstream::NatsJetStreamClient>;

struct SmokeContext {
    prefix: A2aPrefix,
    agent: A2aAgentId,
    jwt: MintedUserJwt,
    client: SmokeClient,
    nats: async_nats::Client,
    op_timeout: Duration,
    expected_caller: String,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    if let Err(err) = match Cli::parse().command {
        Command::MintJwt {
            signing_key_path,
            caller_id,
            audience,
            ttl_secs,
            out,
            creds_out,
        } => mint_jwt(signing_key_path, caller_id, audience, ttl_secs, out, creds_out).await,
        Command::Ed25519Pubkey { seed_hex } => ed25519_pubkey(seed_hex),
        Command::Run {
            profile,
            nats_url,
            caller_jwt,
            agent_id,
            prefix,
            timeout_secs,
        } => run(profile, nats_url, caller_jwt, agent_id, prefix, timeout_secs).await,
    } {
        error!(error = %err, "smoke failed");
        std::process::exit(1);
    }
}

fn ed25519_pubkey(seed_hex: String) -> Result<(), String> {
    let seed = parse_seed_hex(&seed_hex)?;
    let signing_key = SigningKey::from_bytes(&seed);
    println!("{}", hex::encode(signing_key.verifying_key().to_bytes()));
    Ok(())
}

fn parse_seed_hex(raw: &str) -> Result<[u8; 32], String> {
    let trimmed = raw.trim();
    if trimmed.starts_with("0x") || trimmed.starts_with("0X") {
        return Err("seed must not use 0x prefix".into());
    }
    let decoded = hex::decode(trimmed).map_err(|e| format!("invalid seed hex: {e}"))?;
    if decoded.len() != 32 {
        return Err(format!("seed must be 32 bytes, got {}", decoded.len()));
    }
    let mut seed = [0u8; 32];
    seed.copy_from_slice(&decoded);
    Ok(seed)
}

async fn mint_jwt(
    signing_key_path: PathBuf,
    caller_id: String,
    audience: String,
    ttl_secs: u64,
    out: PathBuf,
    creds_out: Option<PathBuf>,
) -> Result<(), String> {
    let seed = std::fs::read_to_string(&signing_key_path)
        .map_err(|e| format!("read signing key {}: {e}", signing_key_path.display()))?
        .trim()
        .to_owned();
    let source = StaticSigningKeySource::new(seed.as_str(), KeyVersion::new("current").map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    let material = source.current().minting_material();
    let user = nkeys::KeyPair::new_user();
    let user_seed = user.seed().map_err(|e| e.to_string())?;
    let subject = UserJwtSubject::from_user_nkey(
        a2a_auth_callout::NkeyPublic::parse(user.public_key()).map_err(|e| e.to_string())?,
    );
    let caller = CallerId::new(caller_id.as_str()).map_err(|e| e.to_string())?;
    let claims = UserJwtClaims {
        kid: KeyVersion::new("current").map_err(|e| e.to_string())?,
        sub: ExternalSubject::new("smoke-external-sub").map_err(|e| e.to_string())?,
        aud: AccountName::new(audience.as_str()),
        data: SpiceDbPrincipal(serde_json::json!({
            "spicedb_subject": format!("user/{caller_id}"),
        })),
        nats_permissions: IssuedPermissions::default_for_caller(&caller),
        caller_id: caller,
    };
    let minted = claims
        .mint(
            &material,
            &subject,
            std::time::SystemTime::now(),
            Duration::from_secs(ttl_secs.max(60)),
        )
        .map_err(|e| e.to_string())?;
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(&out, format!("{}\n", minted.as_str())).map_err(|e| e.to_string())?;
    if let Some(creds_path) = creds_out {
        if let Some(parent) = creds_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let creds = format!(
            "-----BEGIN NATS USER JWT-----\n{}\n-----END NATS USER JWT-----\n-----BEGIN USER NKEY SEED-----\n{}\n-----END USER NKEY SEED-----\n",
            minted.as_str(),
            user_seed
        );
        std::fs::write(&creds_path, creds).map_err(|e| e.to_string())?;
        info!(path = %creds_path.display(), "wrote NATS creds for smoke caller");
    }
    info!(path = %out.display(), "minted smoke caller JWT");
    Ok(())
}

async fn run(
    profile: SmokeProfile,
    nats_url: String,
    caller_jwt: String,
    agent_id: String,
    prefix: String,
    timeout_secs: u64,
) -> Result<(), String> {
    let ctx = build_context(nats_url, caller_jwt, agent_id, prefix, timeout_secs).await?;
    match profile {
        SmokeProfile::Smoke => run_smoke_jwt_path(&ctx).await,
        SmokeProfile::Full => run_full_stack(&ctx).await,
    }
}

async fn build_context(
    nats_url: String,
    caller_jwt: String,
    agent_id: String,
    prefix: String,
    timeout_secs: u64,
) -> Result<SmokeContext, String> {
    let expected_caller = std::env::var("A2A_SMOKE_EXPECTED_CALLER_ID").unwrap_or_else(|_| "smoke-caller-1".into());
    let op_timeout = Duration::from_secs(timeout_secs.max(5));

    let prefix = A2aPrefix::new(prefix).map_err(|e| e.to_string())?;
    let agent = A2aAgentId::new(agent_id).map_err(|e| e.to_string())?;
    let jwt = MintedUserJwt::new(caller_jwt.trim());
    jwt.ensure_fresh().map_err(|e| e.to_string())?;

    let servers: Vec<String> = nats_url
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| {
            if s.starts_with("nats://") || s.starts_with("tls://") {
                s.to_owned()
            } else {
                format!("nats://{s}")
            }
        })
        .collect();
    let servers = if servers.is_empty() {
        vec!["nats://127.0.0.1:4222".into()]
    } else {
        servers
    };

    let nats_config = if let Ok(creds) = std::env::var("NATS_CREDS") {
        NatsConfig::new(servers.clone(), trogon_nats::NatsAuth::Credentials(PathBuf::from(creds)))
    } else if let (Ok(user), Ok(password)) = (
        std::env::var("NATS_USER"),
        std::env::var("NATS_PASSWORD"),
    ) {
        NatsConfig::new(
            servers.clone(),
            trogon_nats::NatsAuth::UserPassword { user, password },
        )
    } else {
        NatsConfig::new(servers.clone(), trogon_nats::NatsAuth::Token(jwt.as_str().to_owned()))
    };
    let nats = trogon_nats::connect(&nats_config, Duration::from_secs(10))
        .await
        .map_err(|e| e.to_string())?;
    let js = trogon_nats::jetstream::NatsJetStreamClient::new(async_nats::jetstream::new(nats.clone()));
    let config = Config::new(prefix.clone(), nats_config);
    let client = Client::new(config, agent.clone(), nats.clone(), js).routing_via_gateway_ingress(jwt.clone());

    Ok(SmokeContext {
        prefix,
        agent,
        jwt,
        client,
        nats,
        op_timeout,
        expected_caller,
    })
}

async fn run_smoke_jwt_path(ctx: &SmokeContext) -> Result<(), String> {
    let send_req = SendMessageRequest {
        message: Some(Message {
            message_id: uuid::Uuid::new_v4().to_string(),
            role: Role::User as i32,
            parts: vec![Part {
                content: Some(a2a_types::part::Content::Text("smoke".into())),
                ..Default::default()
            }],
            ..Default::default()
        }),
        ..Default::default()
    };

    let send_resp = tokio::time::timeout(ctx.op_timeout, ctx.client.message_send(&send_req))
        .await
        .map_err(|_| "message/send timed out".to_string())?
        .map_err(|e| e.to_string())?;
    let task_id = match send_resp.payload {
        Some(a2a_types::send_message_response::Payload::Task(task)) => task.id,
        _ => return Err("message/send did not return a task".into()),
    };
    info!(%task_id, "message/send completed");

    let get_req = GetTaskRequest {
        id: task_id.clone(),
        ..Default::default()
    };
    let task = tokio::time::timeout(ctx.op_timeout, ctx.client.tasks_get(&get_req))
        .await
        .map_err(|_| "tasks/get timed out".to_string())?
        .map_err(|e| e.to_string())?;
    if task.status.as_ref().map(|s| s.state) != Some(TaskState::Completed as i32) {
        return Err(format!("tasks/get task not completed: {:?}", task.status));
    }
    info!(%task_id, "tasks/get completed");

    let audit_subject = format!("{}.audit.ok.tasks.get", ctx.prefix.as_str());
    let mut audit_sub = ctx
        .nats
        .subscribe(async_nats::Subject::from(audit_subject.as_str()))
        .await
        .map_err(|e| e.to_string())?;

    let audit = tokio::time::timeout(Duration::from_secs(15), audit_sub.next())
        .await
        .map_err(|_| "timed out waiting for gateway audit publish".to_string())?
        .ok_or("audit subscription ended".to_string())?;

    let envelope: AuditEnvelopeWire =
        serde_json::from_slice(&audit.payload).map_err(|e| format!("audit json: {e}"))?;
    assert_jwt_audit(&envelope, "tasks/get", &ctx.expected_caller)?;

    println!(
        "SMOKE_OK caller_id={} caller_source=jwt_header",
        ctx.expected_caller
    );
    Ok(())
}

async fn run_full_stack(ctx: &SmokeContext) -> Result<(), String> {
    assert_discovery_operator_signatures()?;
    assert_tier1_declarative_denied(ctx).await?;
    assert_tier3_refusal(ctx).await?;
    let (caller_id, caller_source, rules) = assert_policy_allow_path(ctx).await?;
    println!(
        "SMOKE_FULL_OK caller_id={caller_id} caller_source={caller_source} rules_fired={rules:?}"
    );
    Ok(())
}

fn assert_discovery_operator_signatures() -> Result<(), String> {
    let keys_raw = std::env::var("A2A_DISCOVERY_OPERATOR_KEYS")
        .map_err(|_| "A2A_DISCOVERY_OPERATOR_KEYS missing for full profile".to_string())?;
    let seed_path = std::env::var("A2A_SMOKE_DISCOVERY_OPERATOR_SEED_PATH")
        .map_err(|_| "A2A_SMOKE_DISCOVERY_OPERATOR_SEED_PATH missing".to_string())?;
    let seed_hex = std::fs::read_to_string(&seed_path)
        .map_err(|e| format!("read discovery operator seed: {e}"))?
        .trim()
        .to_owned();
    let seed = parse_seed_hex(&seed_hex)?;
    let signing_key = SigningKey::from_bytes(&seed);

    let _trusted = parse_operator_keys(&keys_raw).map_err(|e| e.to_string())?;
    let key_id = keys_raw
        .split(',')
        .next()
        .and_then(|entry| entry.split_once(':'))
        .map(|(id, _)| id)
        .ok_or("A2A_DISCOVERY_OPERATOR_KEYS empty".to_string())?;
    let key_id = OperatorKeyId::parse(key_id).map_err(|e| e.to_string())?;

    let payload = br#"{"subject":"a2a.discover.>"}"#;
    let now_ms = unix_epoch_ms();
    let envelope = sign_discovery_export(&signing_key, key_id.clone(), payload, now_ms);

    let gate = resolve_operator_signature_gate(&trogon_std::env::SystemEnv, now_ms);
    gate.verify(payload, &envelope)
        .map_err(|e| format!("signed discovery export rejected: {e}"))?;

    let mut bad = envelope.clone();
    bad.signature[0] ^= 0xFF;
    if gate.verify(payload, &bad).is_ok() {
        return Err("unsigned/tampered discovery export was accepted".into());
    }

    info!("discovery operator signature gate verified");
    Ok(())
}

async fn assert_tier1_declarative_denied(ctx: &SmokeContext) -> Result<(), String> {
    let subject =
        compose_gateway_ingress_subject(&ctx.prefix, &ctx.agent, "agent.card").map_err(|e| e.to_string())?;
    let req_id = ReqId::new();
    let params = serde_json::json!({});
    let err = tokio::time::timeout(
        ctx.op_timeout,
        send_unary::<async_nats::Client, serde_json::Value, serde_json::Value>(
            &ctx.nats,
            &subject,
            "agent/card",
            &params,
            &req_id,
            ctx.op_timeout,
            Some(&ctx.jwt),
        ),
    )
    .await
    .map_err(|_| "agent.card gateway request timed out".to_string())?
    .expect_err("agent.card should be denied by tier-1 declarative allowlist");

    let code = json_rpc_code(&err).ok_or_else(|| format!("expected JSON-RPC denial, got {err}"))?;
    if code != -32_801 {
        return Err(format!("tier-1 declarative deny expected -32801, got {code}"));
    }
    info!(code, "tier-1 declarative deny verified");
    Ok(())
}

async fn assert_tier3_refusal(ctx: &SmokeContext) -> Result<(), String> {
    let audit_subject = format!("{}.audit.err.message.send", ctx.prefix.as_str());
    let mut audit_sub = ctx
        .nats
        .subscribe(async_nats::Subject::from(audit_subject.as_str()))
        .await
        .map_err(|e| e.to_string())?;

    let send_req = SendMessageRequest {
        message: Some(Message {
            message_id: uuid::Uuid::new_v4().to_string(),
            role: Role::User as i32,
            parts: vec![Part {
                content: Some(a2a_types::part::Content::Text(TIER3_REFUSE_TRIGGER.into())),
                ..Default::default()
            }],
            ..Default::default()
        }),
        ..Default::default()
    };

    let err = tokio::time::timeout(ctx.op_timeout, ctx.client.message_send(&send_req))
        .await
        .map_err(|_| "tier-3 refusal message/send timed out".to_string())?
        .expect_err("message/send should be refused by tier-3 skill");

    let code = json_rpc_code(&err).ok_or_else(|| format!("expected JSON-RPC refusal, got {err}"))?;
    if code != -32_802 {
        return Err(format!("tier-3 refusal expected -32802, got {code}"));
    }

    let audit = tokio::time::timeout(Duration::from_secs(15), audit_sub.next())
        .await
        .map_err(|_| "timed out waiting for tier-3 refusal audit".to_string())?
        .ok_or("tier-3 refusal audit subscription ended".to_string())?;

    let envelope: AuditEnvelopeWire =
        serde_json::from_slice(&audit.payload).map_err(|e| format!("audit json: {e}"))?;
    if envelope.method != "message/send" {
        return Err(format!("unexpected audit method: {}", envelope.method));
    }
    let refusal = envelope
        .refusal_skill
        .as_deref()
        .ok_or("tier-3 refusal audit missing refusal_skill")?;
    if refusal != "smoke-tier3-refuse" {
        return Err(format!("refusal_skill {refusal:?} != smoke-tier3-refuse"));
    }

    info!(%refusal, "tier-3 refusal verified");
    Ok(())
}

async fn assert_policy_allow_path(ctx: &SmokeContext) -> Result<(String, String, Vec<String>), String> {
    let audit_subject = format!("{}.audit.ok.message.send", ctx.prefix.as_str());
    let mut audit_sub = ctx
        .nats
        .subscribe(async_nats::Subject::from(audit_subject.as_str()))
        .await
        .map_err(|e| e.to_string())?;

    let send_req = SendMessageRequest {
        message: Some(Message {
            message_id: uuid::Uuid::new_v4().to_string(),
            role: Role::User as i32,
            parts: vec![Part {
                content: Some(a2a_types::part::Content::Text("smoke-full-ok".into())),
                ..Default::default()
            }],
            ..Default::default()
        }),
        ..Default::default()
    };

    tokio::time::timeout(ctx.op_timeout, ctx.client.message_send(&send_req))
        .await
        .map_err(|_| "allow-path message/send timed out".to_string())?
        .map_err(|e| e.to_string())?;

    let audit = tokio::time::timeout(Duration::from_secs(15), audit_sub.next())
        .await
        .map_err(|_| "timed out waiting for allow-path audit".to_string())?
        .ok_or("allow-path audit subscription ended".to_string())?;

    let envelope: AuditEnvelopeWire =
        serde_json::from_slice(&audit.payload).map_err(|e| format!("audit json: {e}"))?;
    assert_jwt_audit(&envelope, "message/send", &ctx.expected_caller)?;

    let rules = envelope
        .rules_fired
        .clone()
        .ok_or("allow-path audit missing rules_fired")?;
    let rules_joined = rules.join(",");
    if !rules_joined.contains("gateway.tier1.spicedb_allowed") {
        return Err(format!("expected gateway.tier1.spicedb_allowed in rules_fired: {rules:?}"));
    }
    if !rules_joined.contains("gateway.tier1.declarative") {
        return Err(format!("expected tier-1 declarative rule in rules_fired: {rules:?}"));
    }
    if !rules_joined.contains("gateway.tier3.evaluated_allow") && !rules_joined.contains("gateway.tier3.redacted")
    {
        return Err(format!("expected tier-3 allow in rules_fired: {rules:?}"));
    }

    let caller_id = envelope.caller_id.clone().unwrap_or_default();
    let caller_source = envelope.caller_source.clone().unwrap_or_default();
    Ok((caller_id, caller_source, rules))
}

fn normalize_audit_caller_id(caller_id: &str) -> &str {
    caller_id
        .split_once('/')
        .map(|(_, slug)| slug)
        .unwrap_or(caller_id)
}

fn assert_jwt_audit(envelope: &AuditEnvelopeWire, method: &str, expected_caller: &str) -> Result<(), String> {
    if envelope.method != method {
        return Err(format!("unexpected audit method: {}", envelope.method));
    }
    let caller_id = envelope.caller_id.as_deref().unwrap_or("_");
    let caller_source = envelope.caller_source.as_deref().unwrap_or("");
    if normalize_audit_caller_id(caller_id) != expected_caller {
        return Err(format!(
            "audit caller_id {caller_id:?} != expected {expected_caller} (source={caller_source:?})"
        ));
    }
    if caller_source != "jwt_header" {
        return Err(format!("audit caller_source {caller_source:?} != jwt_header"));
    }
    if caller_id == "_" {
        return Err("audit used fallback caller_id".into());
    }
    Ok(())
}

fn json_rpc_code(err: &ClientError) -> Option<i32> {
    match err {
        ClientError::JsonRpc { code, .. } => Some(*code),
        _ => None,
    }
}

fn unix_epoch_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod wire_json_shape {
    use a2a_types::part::Content;
    use a2a_types::{Message, Part, Role, SendMessageRequest};

    #[test]
    fn message_send_params_json_shape() {
        let req = SendMessageRequest {
            message: Some(Message {
                message_id: "m1".into(),
                role: Role::User as i32,
                parts: vec![Part {
                    content: Some(Content::Text("SMOKE_T3_REFUSE_ME".into())),
                    ..Default::default()
                }],
                ..Default::default()
            }),
            ..Default::default()
        };
        let params = serde_json::to_value(&req).expect("params");
        eprintln!("params={}", serde_json::to_string_pretty(&params).unwrap());
        assert!(
            params.pointer("/message/parts/0/text").is_some()
                || params.pointer("/message/parts/0/content/text").is_some()
        );
    }
}
