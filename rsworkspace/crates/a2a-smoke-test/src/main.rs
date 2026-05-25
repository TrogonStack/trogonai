use std::path::PathBuf;
use std::time::Duration;

use a2a_auth_callout::{
    jwt::ExternalSubject, AccountName, CallerId, MintedUserJwt, SpiceDbPrincipal, UserJwtClaims, UserJwtSubject,
};
use a2a_auth_callout::permissions::IssuedPermissions;
use a2a_auth_callout::signing_key_source::{KeyVersion, SigningKeySource, StaticSigningKeySource};
use trogon_nats::NatsAuth;
use a2a_nats::client::Client;
use a2a_nats::{A2aAgentId, A2aPrefix, Config, NatsConfig};
use a2a_types::{GetTaskRequest, Message, Part, Role, SendMessageRequest, TaskState};
use clap::{Parser, Subcommand};
use futures::StreamExt;
use serde::Deserialize;
use tokio::time::timeout;
use tracing::{error, info};

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
    /// End-to-end smoke: message/send + tasks/get via gateway ingress; assert JWT audit attribution.
    Run {
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
struct AuditEnvelopeWire {
    #[serde(default)]
    caller_id: Option<String>,
    #[serde(default)]
    caller_source: Option<String>,
    method: String,
    #[allow(dead_code)]
    outcome: serde_json::Value,
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
        Command::Run {
            nats_url,
            caller_jwt,
            agent_id,
            prefix,
            timeout_secs,
        } => run_smoke(nats_url, caller_jwt, agent_id, prefix, timeout_secs).await,
    } {
        error!(error = %err, "smoke failed");
        std::process::exit(1);
    }
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
        data: SpiceDbPrincipal(serde_json::json!({ "spicedb_subject": caller_id })),
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

async fn run_smoke(
    nats_url: String,
    caller_jwt: String,
    agent_id: String,
    prefix: String,
    timeout_secs: u64,
) -> Result<(), String> {
    let expected_caller = std::env::var("A2A_SMOKE_EXPECTED_CALLER_ID").unwrap_or_else(|_| "smoke-caller-1".into());
    let op_timeout = Duration::from_secs(timeout_secs.max(5));

    let prefix = A2aPrefix::new(prefix).map_err(|e| e.to_string())?;
    let agent = A2aAgentId::new(agent_id).map_err(|e| e.to_string())?;
    let jwt = MintedUserJwt::new(caller_jwt.trim());
    jwt.ensure_fresh().map_err(|e| e.to_string())?;

    let mut servers: Vec<String> = nats_url
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
    if servers.is_empty() {
        servers.push("nats://127.0.0.1:4222".into());
    }

    let nats_config = if let Ok(creds) = std::env::var("NATS_CREDS") {
        NatsConfig::new(servers.clone(), NatsAuth::Credentials(PathBuf::from(creds)))
    } else if let (Ok(user), Ok(password)) = (
        std::env::var("NATS_USER"),
        std::env::var("NATS_PASSWORD"),
    ) {
        NatsConfig::new(
            servers.clone(),
            NatsAuth::UserPassword { user, password },
        )
    } else {
        NatsConfig::new(servers.clone(), NatsAuth::Token(jwt.as_str().to_owned()))
    };
    let nats = trogon_nats::connect(&nats_config, Duration::from_secs(10))
        .await
        .map_err(|e| e.to_string())?;
    let js = trogon_nats::jetstream::NatsJetStreamClient::new(async_nats::jetstream::new(nats.clone()));

    let audit_subject = format!("{}.audit.ok.tasks.get", prefix.as_str());
    let mut audit_sub = nats
        .subscribe(async_nats::Subject::from(audit_subject.as_str()))
        .await
        .map_err(|e| e.to_string())?;

    let config = Config::new(prefix.clone(), nats_config);
    let client = Client::new(config, agent.clone(), nats.clone(), js).routing_via_gateway_ingress(jwt);

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

    let send_resp = timeout(op_timeout, client.message_send(&send_req))
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
    let task = timeout(op_timeout, client.tasks_get(&get_req))
        .await
        .map_err(|_| "tasks/get timed out".to_string())?
        .map_err(|e| e.to_string())?;
    if task.status.as_ref().map(|s| s.state) != Some(TaskState::Completed as i32) {
        return Err(format!("tasks/get task not completed: {:?}", task.status));
    }
    info!(%task_id, "tasks/get completed");

    let audit = timeout(Duration::from_secs(15), audit_sub.next())
        .await
        .map_err(|_| "timed out waiting for gateway audit publish".to_string())?
        .ok_or("audit subscription ended".to_string())?;

    let envelope: AuditEnvelopeWire =
        serde_json::from_slice(&audit.payload).map_err(|e| format!("audit json: {e}"))?;
    if envelope.method != "tasks/get" {
        return Err(format!("unexpected audit method: {}", envelope.method));
    }
    let caller_id = envelope.caller_id.as_deref().unwrap_or("_");
    let caller_source = envelope.caller_source.as_deref().unwrap_or("");
    if caller_id != expected_caller.as_str() {
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

    info!(
        caller_id = %caller_id,
        caller_source = %caller_source,
        "gateway JWT path verified"
    );
    println!("SMOKE_OK caller_id={caller_id} caller_source={caller_source}");
    Ok(())
}
