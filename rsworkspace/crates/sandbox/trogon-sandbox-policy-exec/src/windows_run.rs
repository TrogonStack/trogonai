//! Runs one command under the Windows network fence: the
//! egress proxy listens on a loopback port the fence permits, and the command
//! is started through `trogon-sandbox-fence-helper exec` with the filesystem rules of the
//! settings.

use std::path::{Path, PathBuf};

use trogon_sandbox_contract::{FsPathEntry, SecretMode};
use trogon_sandbox_subprocess_worker::attribution::CommandAttribution;
use trogon_sandbox_subprocess_worker::fenced_proxy::{FencedProxy, FencedSecrets};
use trogon_sandbox_subprocess_worker::lease_proxy::CaFilePaths;
use trogon_sandbox_subprocess_worker::tool_proxy_env::ToolProxyEnv;
use trogon_sandbox_subprocess_worker::windows_ca::{TrustBundle, persistent_ca_files};
use trogon_sandbox_subprocess_worker::windows_fs::RealDisk;
use trogon_sandbox_windows_fence::{
    Access, BinShell, DEFAULT_PROXY_PORT_RANGE, FenceHelper, FsAccessSet, HostDisk, InstallContext, ProfileExclusions,
    SandboxStatus, VerifyOptions, WindowsFence, check_capabilities, check_dependencies, exclusion_conflict_error,
    profile_exclusion_conflicts,
};
use trogon_std::env::{ReadEnv, SystemEnv};

use crate::command_overrides::CommandOverrides;
use crate::config::PolicyExecConfig;
use crate::credentials::PreparedCredentials;
use crate::error::RuntimeError;
use crate::settings::{EffectiveCredentials, SandboxSettings};
use crate::tls_intercept::requires_tls_intercept;
use crate::windows::{SessionStamp, WindowsLaunch, masked_file_denies, session_access, session_request};
use crate::windows_run_handle::WindowsRunHandle;
use trogon_sandbox_egress_proxy::CommandId;

/// What a run needs besides the settings.
#[derive(Debug, Clone)]
pub struct WindowsRun {
    /// The `trogon-sandbox-fence-helper` executable; the settings' `windows.fenceHelper.path` wins.
    pub fence_helper: PathBuf,
    pub command: String,
    pub cwd: PathBuf,
    /// Profile exclusions the session may still be granted read on (the
    /// opt-in to profile exclusions). Never applies to a write
    /// root or to a path above an exclusion.
    pub allow_profile_exclusions: Vec<String>,
    /// The overrides for this command.
    pub overrides: CommandOverrides,
    /// Lets the caller update the network policy of the running command and
    /// stop it.
    pub handle: Option<WindowsRunHandle>,
    /// The command's own environment, which a credential binding reads before
    /// the runtime's and this process's.
    pub env: Vec<(String, String)>,
    /// The key the proxy user name carries instead of the command text.
    pub command_id: Option<CommandId>,
}

fn launch_error(error: impl Into<anyhow::Error>) -> RuntimeError {
    RuntimeError::Launch(error.into())
}

fn plain(path: &std::path::Path) -> String {
    let text = path.to_string_lossy();
    text.strip_prefix(r"\\?\").unwrap_or(&text).to_string()
}

/// The CA the proxy signs with and the bundle the command trusts.
struct SessionCa {
    ca: CaFilePaths,
    bundle: TrustBundle,
}

/// The CA choice of a Windows session: explicit `caCertPath`/`caKeyPath` are
/// used as they are, otherwise the persistent CA under the state directory
/// is ensured and trusted by the sandbox user.
fn session_ca(
    settings: &SandboxSettings,
    config: &PolicyExecConfig,
    fence: &WindowsFence,
    status: &SandboxStatus,
    masking: bool,
) -> Result<Option<SessionCa>, RuntimeError> {
    let options = config.configured_network_options();
    if !masking && !settings.tls_terminates() && !requires_tls_intercept(&config.policy().network) {
        return Ok(None);
    }
    let ca = match &options.ca_files {
        Some(files) => CaFilePaths {
            cert: files.cert_path().to_path_buf(),
            key: files.key_path().to_path_buf(),
        },
        None => persistent_ca_files(fence, status).map_err(launch_error)?,
    };
    let bundle = TrustBundle::write(&ca.cert, options.extra_ca_cert_paths.as_slice()).map_err(launch_error)?;
    Ok(Some(SessionCa { ca, bundle }))
}

/// What the recorded profile read grant excludes, when there is one: a grant
/// that cannot be used stops the run.
fn profile_exclusions(fence: &WindowsFence, status: &SandboxStatus) -> Result<ProfileExclusions, RuntimeError> {
    if status.user.profile_grant.is_none() {
        return Ok(ProfileExclusions::default());
    }
    let grant = fence.profile_read_status().map_err(launch_error)?;
    if let Some(error) = grant.unusable() {
        return Err(launch_error(error));
    }
    Ok(grant.exclusions)
}

/// No path the session grants, in any form, may reach what the profile read
/// grant excludes.
fn check_profile_exclusions(
    exclusions: &ProfileExclusions,
    access: &FsAccessSet,
    session_ca: Option<&SessionCa>,
    admitted: &[String],
) -> Result<(), RuntimeError> {
    let own_read: Vec<String> = session_ca
        .map(|session| plain(session.bundle.dir()))
        .into_iter()
        .collect();
    let refused: Vec<_> = [
        profile_exclusion_conflicts(&access.grant_read, exclusions, Access::Read, admitted, &HostDisk),
        profile_exclusion_conflicts(&own_read, exclusions, Access::Read, &[], &HostDisk),
        profile_exclusion_conflicts(&access.grant_write, exclusions, Access::Write, &[], &HostDisk),
    ]
    .concat();
    if refused.is_empty() {
        return Ok(());
    }
    Err(launch_error(exclusion_conflict_error(&refused)))
}

/// Runs the command to completion and returns its exit code. The egress
/// proxy ends with the call.
pub async fn run(settings: &SandboxSettings, run: WindowsRun) -> Result<i32, RuntimeError> {
    let _finished = run.handle.as_ref().map(WindowsRunHandle::finish_on_drop);
    let windows = settings
        .windows()
        .map_err(|error| RuntimeError::Invalid(anyhow::anyhow!(error.message().to_string())))?;
    let fence_helper_path = windows
        .fence_helper_path
        .clone()
        .unwrap_or_else(|| run.fence_helper.clone());
    let fence_helper = FenceHelper::new(fence_helper_path).map_err(launch_error)?;

    let config = settings
        .to_standard_config(&run.cwd)
        .map_err(|error| RuntimeError::Invalid(anyhow::anyhow!(error.message().to_string())))?;
    let fence = WindowsFence::new(fence_helper.clone());
    let status = fence.status(windows.sublayer_guid.as_ref()).map_err(launch_error)?;
    let ports = windows.proxy_port_range.unwrap_or(DEFAULT_PROXY_PORT_RANGE);
    let dependencies = check_dependencies(
        &status.user,
        &status.wfp,
        &InstallContext {
            sublayer_guid: windows.sublayer_guid.clone(),
            proxy_port_range: windows.proxy_port_range,
            sandbox_user: windows.sandbox_user.clone(),
            allow_loopback: settings.allow_local_binding(),
        },
    );
    for warning in &dependencies.warnings {
        tracing::warn!("{warning}");
    }
    if let Some(error) = dependencies.error() {
        return Err(launch_error(error));
    }
    check_capabilities(&status.user).map_err(launch_error)?;
    let sandbox_sid = match (&status.user.sid, status.user.provisioned) {
        (Some(sid), true) => sid.clone(),
        _ => {
            return Err(RuntimeError::Unsupported(
                "the sandbox account is not provisioned; run windows-install first".to_string(),
            ));
        }
    };

    let own = run.overrides.effective_credentials(&config)?;
    let masking = own
        .as_ref()
        .map_or(config.bindings(), |own| own.rules.bindings.as_slice())
        .iter()
        .any(|binding| binding.mode() == SecretMode::Mask);
    let session_ca = session_ca(settings, &config, &fence, &status, masking)?;
    let policy = config.policy().fs.clone().unwrap_or_default();
    let masked = masked_file_denies(config.bindings(), &|path| std::fs::read_to_string(path))?;
    let disabled = config.configured_platform_options().filesystem.disabled.is_disabled();
    let mut credential_deny = if disabled {
        Vec::new()
    } else {
        settings.credential_deny_paths().to_vec()
    };
    credential_deny.extend(masked.iter().cloned());
    let access = session_access(&policy, &credential_deny, disabled, &RealDisk)?;
    let exclusions = profile_exclusions(&fence, &status)?;
    check_profile_exclusions(&exclusions, &access, session_ca.as_ref(), &run.allow_profile_exclusions)?;
    let mut stamped = access.clone();
    if let Some(session) = &session_ca {
        stamped.grant_read.push(plain(session.bundle.dir()));
    }
    let holder = std::process::id();
    let granted = fence.acl_grant_persistent(&sandbox_sid, holder, &stamped, &run.allow_profile_exclusions);
    let verified = granted.and_then(|()| {
        fence
            .verify(&VerifyOptions {
                proxy_port_range: Some(ports),
                allow_loopback: settings.allow_local_binding(),
                sublayer_guid: windows.sublayer_guid.clone(),
                sandbox_user: windows.sandbox_user.clone(),
                write_roots: access.grant_write.clone(),
                ..VerifyOptions::default()
            })
            .map(|result| {
                for warning in &result.warnings {
                    tracing::warn!("{warning}");
                }
            })
    });
    let outcome = match verified {
        Ok(()) => {
            run_granted(
                settings,
                &config,
                &fence_helper,
                &run,
                &access,
                &masked,
                own.as_ref(),
                session_ca.as_ref(),
                ports,
            )
            .await
        }
        Err(error) => Err(launch_error(error)),
    };
    release_acl(&fence, &sandbox_sid, holder);
    outcome
}

fn release_acl(fence: &WindowsFence, sandbox_sid: &str, holder: u32) {
    if let Err(reason) = fence.acl_release(sandbox_sid, holder) {
        tracing::warn!(%reason, "the session file access was not fully released");
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_granted(
    settings: &SandboxSettings,
    config: &PolicyExecConfig,
    fence_helper: &FenceHelper,
    run: &WindowsRun,
    access: &FsAccessSet,
    masked: &[FsPathEntry],
    own: Option<&EffectiveCredentials>,
    session_ca: Option<&SessionCa>,
    ports: trogon_sandbox_windows_fence::ProxyPortRange,
) -> Result<i32, RuntimeError> {
    let credentials = PreparedCredentials::prepare(config, own, &run.env, &config.policy().network).await?;
    let secrets = credentials.registry().map(|(registry, sigv4)| FencedSecrets {
        registry,
        sigv4,
        plaintext_inject: credentials.plaintext_inject(),
    });
    let proxy = FencedProxy::start_with(
        &config.policy().network,
        ports,
        session_ca.map(|session| &session.ca),
        secrets,
    )
    .await
    .map_err(RuntimeError::Proxy)?
    .attributed_to(CommandAttribution::for_command(
        &run.command,
        run.command_id.as_ref().map(CommandId::as_str),
    ));
    if let Some(handle) = &run.handle {
        handle.serve(proxy.policy_handle());
    }

    let env = SystemEnv;
    let system_root = env.var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_string());
    let bin_shell = BinShell::parse(None, &system_root).map_err(launch_error)?;
    let base_env = ["PATH", "PATHEXT"]
        .into_iter()
        .filter_map(|name| Some((name.to_string(), env.var(name).ok()?)))
        .collect();
    let launch = WindowsLaunch {
        command: run.command.clone(),
        bin_shell,
        held_by: Some(std::process::id()),
        base_env,
        mask_env: credentials.sentinel_env().to_vec(),
        proxy_env: std::iter::once(("TROGON_SANDBOXED".to_string(), "1".to_string()))
            .chain(
                match session_ca {
                    Some(session) => proxy.environment_trusting(session.bundle.path().to_path_buf()),
                    None => proxy.environment(),
                }
                .with_tools(ToolProxyEnv::enabled(
                    settings.java_agent_jar_path().map(Path::to_path_buf),
                ))
                .vars(),
            )
            .collect(),
        allow_loopback: settings.allow_local_binding(),
        cwd: plain(&run.cwd),
        ca_cert_path: session_ca.map(|session| plain(session.bundle.path()).replace('\\', "/")),
    };
    let (command_masked, command_credential_deny) = match &own {
        Some(own) => (
            masked_file_denies(&own.rules.bindings, &|path| std::fs::read_to_string(path))?,
            if run.overrides.credentials.is_some() {
                own.deny.clone()
            } else {
                Vec::new()
            },
        ),
        None => (masked.to_vec(), Vec::new()),
    };
    let stamp = SessionStamp {
        access,
        masked: &command_masked,
        credential_deny: &command_credential_deny,
        verified_loopback: settings.allow_local_binding(),
    };
    let args = session_request(config, launch, &stamp, &run.overrides, &RealDisk)?
        .args()
        .map_err(launch_error)?;

    let fence_helper = match &run.overrides.fence_helper {
        Some(path) => {
            let own = FenceHelper::new(path.clone()).map_err(launch_error)?;
            let windows = settings
                .windows()
                .map_err(|error| RuntimeError::Invalid(anyhow::anyhow!(error.message().to_string())))?;
            let reported = WindowsFence::new(own.clone())
                .status(windows.sublayer_guid.as_ref())
                .map_err(launch_error)?;
            check_capabilities(&reported.user).map_err(launch_error)?;
            own
        }
        None => fence_helper.clone(),
    };
    let mut command = fence_helper.command(&args);
    command.current_dir(&run.cwd);
    let mut interrupt = crate::windows_wait::Interrupt::register().map_err(RuntimeError::Io)?;
    let mut child = tokio::process::Command::from(command)
        .kill_on_drop(true)
        .spawn()
        .map_err(RuntimeError::Io)?;
    let stop = async {
        match &run.handle {
            Some(handle) => handle.terminated().await,
            None => std::future::pending().await,
        }
    };
    let outcome = crate::windows_wait::wait_forwarding(&mut child, async {
        tokio::select! {
            () = interrupt.arrived() => {}
            () = stop => {}
        }
    })
    .await
    .map_err(RuntimeError::Io)?;
    proxy.shutdown().await;
    Ok(outcome.exit_code())
}
