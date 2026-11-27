//! Runs the TypeScript language server inside a sandbox and asks it for the symbols in a file and
//! the completions at a cursor position.
//!
//! Needs `NATS_URL`, `SANDBOX_API_KEY` and `SANDBOX_TENANT`.

mod constants;

use std::collections::BTreeMap;
use std::io::Write;
use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use constants::{INSTALL_TIMEOUT_SECONDS, PROJECT_DIR, SOURCE};
use trogon_sandbox_client::{CommandOptions, SandboxClient};
use trogon_sandbox_contract::{
    BootMode, LeaseId, LeasePersistence, LeasePrewarm, LeaseRequest, LeaseRequestKey, LeaseTtl, NetworkPolicy,
    OciImageRef, PlatformOptions, PrivilegedAccess, ProviderKind, RetentionToggles, SandboxFileWrite, SandboxImage,
    SandboxSpec, SandboxSpecParts, SandboxTags, SecretBindings, TenantId, WorkspaceSource,
};
use trogon_sandbox_lsp::{DocumentPath, LspPresetName, LspServer, LspServerOptions, Position, ProjectDir};
use trogon_std::env::{ReadEnv, SystemEnv};

fn env(name: &str) -> Result<String> {
    SystemEnv.var(name).with_context(|| format!("{name} must be set"))
}

#[tokio::main]
async fn main() -> Result<()> {
    let nats_url = env("NATS_URL")?;
    let api_key = env("SANDBOX_API_KEY")?;
    let tenant = TenantId::new(env("SANDBOX_TENANT")?)?;

    let client = Arc::new(
        SandboxClient::connect(&nats_url)
            .await
            .context("connecting to NATS")?
            .with_api_key(api_key),
    );

    let granted = client
        .lease(&lease_request(tenant.clone())?, Duration::from_secs(120))
        .await
        .context("leasing a sandbox")?;
    let lease_id = granted.lease_id;

    let result = run(&client, &lease_id).await;

    if client.delete_lease(tenant, &lease_id).await.is_err() {
        let _ = client.cancel(&lease_id).await;
    }
    result
}

fn lease_request(tenant: TenantId) -> Result<LeaseRequest> {
    let key = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let spec = SandboxSpec::new(SandboxSpecParts {
        image: SandboxImage::Registry {
            reference: OciImageRef::parse_unqualified("node:22-bookworm-slim"),
            pull_secret: None,
        },
        workspace: WorkspaceSource::empty(),
        env: BTreeMap::new(),
        cpu_seconds: NonZeroU32::new(3600).context("cpu seconds")?,
        memory_mib: NonZeroU32::new(512).context("memory")?,
        snapshot_out: None,
        network_policy: Some(NetworkPolicy::AllowAll),
        mounts: Vec::new(),
        drives: Vec::new(),
        exposed_ports: Vec::new(),
        boot_mode: BootMode::Bare,
        working_dir: None,
        user: None,
        privileged: PrivilegedAccess::default(),
        wasi_capabilities: None,
        fs_policy: None,
        secret_bindings: SecretBindings::default(),
        platform: PlatformOptions::default(),
        idle_timeout_seconds: None,
        no_ttl: LeaseTtl::default(),
        idle_policy: None,
        vcpu_millis: None,
        disk_mib: None,
        retention: RetentionToggles::default(),
    })?;
    Ok(LeaseRequest {
        operation_id: None,
        tenant,
        provider_pref: vec![ProviderKind::LocalDocker],
        spec,
        ttl_seconds: 300,
        idempotency_key: LeaseRequestKey::new(format!("sandbox-lsp-example-{key}"))?,
        isolation_tier: None,
        host_imports: Vec::new(),
        required_capabilities: None,
        prewarm: LeasePrewarm::Regular,
        tags: SandboxTags::default(),
        name: None,
        persistent: LeasePersistence::Persistent,
        native: None,
        region: None,
        fallback_regions: Vec::new(),
    })
}

async fn run(client: &Arc<SandboxClient>, lease_id: &LeaseId) -> Result<()> {
    shell(client, lease_id, &format!("mkdir -p {PROJECT_DIR}"), None, 60).await?;
    shell(
        client,
        lease_id,
        "npm install -g typescript-language-server && npm install typescript@5.9.3",
        Some(PROJECT_DIR),
        INSTALL_TIMEOUT_SECONDS,
    )
    .await?;

    client
        .write_files(
            lease_id,
            vec![SandboxFileWrite {
                path: format!("{PROJECT_DIR}/main.ts"),
                content: SOURCE.as_bytes().to_vec(),
                mode: None,
            }],
            None,
        )
        .await?;

    let lsp = LspServer::in_sandbox(
        Arc::clone(client),
        lease_id.clone(),
        LspPresetName::TypeScript,
        ProjectDir::new(PROJECT_DIR)?,
        LspServerOptions::default(),
    );
    lsp.start().await?;
    let outcome = query(&lsp).await;
    lsp.stop().await;
    outcome
}

async fn query(lsp: &LspServer) -> Result<()> {
    let main = DocumentPath::new("main.ts")?;
    lsp.did_open(&main, SOURCE, None).await?;

    let symbols = lsp.document_symbols(&main).await?;
    let names = symbols.as_ref().map(|s| s.names()).unwrap_or_default();
    writeln!(std::io::stdout(), "Symbols: {names:?}")?;

    let completions = lsp.completions(&main, Position::new(5, 8)).await?;
    let labels: Vec<&str> = completions.iter().take(10).map(|item| item.label.as_str()).collect();
    writeln!(std::io::stdout(), "Completions: {labels:?}")?;
    Ok(())
}

/// Runs a shell command and fails unless it exits with code 0.
async fn shell(
    client: &SandboxClient,
    lease_id: &LeaseId,
    script: &str,
    cwd: Option<&str>,
    timeout_seconds: u32,
) -> Result<()> {
    let options = CommandOptions {
        cwd: cwd.map(str::to_string),
        timeout_seconds,
        ..CommandOptions::default()
    };
    let output = client
        .run_command(lease_id, vec!["sh".into(), "-c".into(), script.into()], options)
        .await?;
    if output.exit_code != 0 {
        bail!(
            "`{script}` failed with exit code {}: {}",
            output.exit_code,
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}
