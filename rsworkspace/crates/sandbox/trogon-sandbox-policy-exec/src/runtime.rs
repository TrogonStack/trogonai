use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use nix::sys::signal::Signal;
use nix::unistd::Pid;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use trogon_sandbox_contract::NetworkOptions;
use trogon_sandbox_contract::{DomainPattern, FsPathEntry, FsPolicy, UnlistedDestinations};
use trogon_sandbox_contract::{DomainRule, DomainRuleAction, NetworkPolicy, OpaqueTunnelPolicy};
use trogon_sandbox_egress_proxy::{
    AskCallback, CommandId, CredentialPlacement, DeciderLimits, DeciderTransport, DenialReasons, HeaderMutator,
    PolicyHandle, RequestCallback, TunnelGate, ViolationFilter, ViolationStore,
};
use trogon_sandbox_protocol::CappedOutput;
use trogon_sandbox_subprocess_worker::attribution::CommandAttribution;
#[cfg(target_os = "linux")]
use trogon_sandbox_subprocess_worker::fs_violation::FsDenialSink;
#[cfg(target_os = "linux")]
use trogon_sandbox_subprocess_worker::isolation::BubblewrapBackend;
#[cfg(target_os = "macos")]
use trogon_sandbox_subprocess_worker::isolation::SeatbeltBackend;
use trogon_sandbox_subprocess_worker::isolation::limits::LimitGuard;
use trogon_sandbox_subprocess_worker::isolation::{
    ChildStdio, HostView, LaunchedProcess, MonitorGuard, PassthroughBackend,
};
use trogon_sandbox_subprocess_worker::placeholders::{PlaceholderHold, PlaceholderLedger};
use trogon_sandbox_subprocess_worker::proxy_env::CaCertificate;
use trogon_sandbox_subprocess_worker::tool_proxy_env::{
    JAVA_TOOL_OPTIONS, PREFER_IPV4_STACK, ToolProxyEnv, java_tool_options,
};
use trogon_sandbox_subprocess_worker::{EgressPolicy, ExtraBind, IsolationBackend, IsolationBackendKind, LaunchSpec};

use crate::command::{CommandOutput, CommandSpec, Truncation};
use crate::config::{FilesystemView, PolicyExecConfig, RuntimePolicy, validate_policy};
use crate::constants::MIN_COMMAND_ID_LEN;
use crate::constants::{GUEST_WORKSPACE, PIPE_DRAIN_GRACE, RESET_POLL, RESET_WAIT};
use crate::credentials::{CommandCredentials, PreparedCredentials};
use crate::error::RuntimeError;
use crate::host_env;
use crate::proxy::{Callbacks, CommandProxy, RequestDeciderSetup, replaces_runtime_proxy};
use crate::restrictions::literal_entry;
use crate::settings::{CredentialContext, SandboxSettings, SettingsError};
use crate::spawn::{CommandExit, RunningCommand};

mod kernel_denials;

use kernel_denials::KernelDenials;

/// Domains one command may reach beyond its `CommandSpec`: the library
/// counterpart of a per-command allow list.
#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct CommandNetworkLists {
    pub allowed_domains: Vec<DomainPattern>,
}

impl CommandNetworkLists {
    pub fn new(allowed_domains: Vec<DomainPattern>) -> Self {
        Self { allowed_domains }
    }

    /// Reads the lists of an untyped document: a missing or null
    /// `allowedDomains` is empty, anything but an array is refused, and one
    /// entry that is not a valid string pattern refuses the whole document,
    /// so nothing is registered.
    pub fn from_json(lists: &serde_json::Value) -> Result<Self, RuntimeError> {
        let entries = match lists.get("allowedDomains") {
            None | Some(serde_json::Value::Null) => return Ok(Self::default()),
            Some(serde_json::Value::Array(entries)) => entries,
            Some(_) => {
                return Err(RuntimeError::Invalid(anyhow::anyhow!(
                    "command network lists: allowedDomains must be an array of strings."
                )));
            }
        };
        let mut allowed_domains = Vec::with_capacity(entries.len());
        for entry in entries {
            match entry.as_str().map(DomainPattern::new) {
                Some(Ok(pattern)) => allowed_domains.push(pattern),
                _ => {
                    return Err(RuntimeError::Invalid(anyhow::anyhow!(
                        "command network lists: invalid allowedDomains entry {entry}. {}",
                        trogon_sandbox_protocol::ALLOWED_DOMAIN_ENTRY_MESSAGE
                    )));
                }
            }
        }
        Ok(Self { allowed_domains })
    }
}

/// What a command adds to the lists of the filesystem policy it runs under.
#[derive(Default)]
struct FsAdditions {
    allow_read_within_deny: Vec<FsPathEntry>,
    own_allow_within_deny: Vec<FsPathEntry>,
    allow_write: Vec<FsPathEntry>,
    deny_write: Vec<FsPathEntry>,
}

impl FsAdditions {
    fn applied_to(self, fs: &FsPolicy) -> Result<FsPolicy, RuntimeError> {
        let joined =
            |current: &[FsPathEntry], added: Vec<FsPathEntry>| current.iter().cloned().chain(added).collect::<Vec<_>>();
        FsPolicy::new(
            fs.deny_read().to_vec(),
            fs.credential_deny_read().to_vec(),
            joined(fs.allow_read_within_deny(), self.allow_read_within_deny),
            joined(fs.own_allow_within_deny(), self.own_allow_within_deny),
            joined(fs.allow_write(), self.allow_write),
            joined(fs.deny_write(), self.deny_write),
        )
        .map_err(|err| RuntimeError::Invalid(anyhow::Error::new(err)))
    }
}

struct ActiveCommand {
    cancel: CancellationToken,
    follows_network: bool,
    policy: Option<PolicyHandle>,
    pid: Option<u32>,
    launching: bool,
    pending_signals: Vec<Signal>,
    spec_allow: Vec<DomainPattern>,
    extra_rules: Vec<DomainRule>,
    credentials: CommandCredentials,
}

struct Launched {
    child: tokio::process::Child,
    limits: LimitGuard,
    placeholders: PlaceholderHold,
    monitors: MonitorGuard,
    denials: Option<KernelDenials>,
}

struct Started {
    command: CommandId,
    cancel: CancellationToken,
    child: tokio::process::Child,
    limits: Option<LimitGuard>,
    placeholders: PlaceholderHold,
    monitors: MonitorGuard,
    denials: Option<KernelDenials>,
    proxy: Option<CommandProxy>,
    _credentials: PreparedCredentials,
}

impl Started {
    async fn release_process(&mut self) {
        if let Some(limits) = self.limits.take() {
            limits.release().await;
        }
        self.placeholders.release_now();
    }
}

async fn supervise(
    child: &mut tokio::process::Child,
    cancel: &CancellationToken,
    timeout: Option<Duration>,
) -> (std::io::Result<std::process::ExitStatus>, bool) {
    let expiry = async {
        match timeout {
            Some(timeout) => tokio::time::sleep(timeout).await,
            None => std::future::pending().await,
        }
    };
    tokio::select! {
        status = child.wait() => (status, false),
        () = cancel.cancelled() => {
            let _ = child.kill().await;
            (child.wait().await, false)
        }
        () = expiry => {
            let _ = child.kill().await;
            (child.wait().await, true)
        }
    }
}

fn command_cwd(config: &PolicyExecConfig, spec: &CommandSpec) -> PathBuf {
    match &spec.cwd {
        Some(cwd) if cwd.is_absolute() => cwd.clone(),
        Some(cwd) => config.workspace_path().join(cwd),
        None => config.workspace_path().to_path_buf(),
    }
}

fn merge_command_rules(network: &NetworkPolicy, extra: &[DomainRule]) -> NetworkPolicy {
    if extra.is_empty() {
        return network.clone();
    }
    match network {
        NetworkPolicy::UserDefined {
            rules,
            allow_ranges,
            deny_ranges,
            denied_domains,
            opaque_tunnels,
            request_rules,
        } => NetworkPolicy::UserDefined {
            rules: rules.iter().chain(extra).cloned().collect(),
            allow_ranges: allow_ranges.clone(),
            deny_ranges: deny_ranges.clone(),
            denied_domains: denied_domains.clone(),
            opaque_tunnels: *opaque_tunnels,
            request_rules: request_rules.clone(),
        },
        NetworkPolicy::DenyAll => NetworkPolicy::UserDefined {
            rules: extra.to_vec(),
            allow_ranges: Vec::new(),
            deny_ranges: Vec::new(),
            denied_domains: Vec::new(),
            opaque_tunnels: OpaqueTunnelPolicy::Allow,
            request_rules: Vec::new(),
        },
        NetworkPolicy::AllowAll => NetworkPolicy::AllowAll,
    }
}

struct Live {
    config: Arc<PolicyExecConfig>,
    #[cfg(target_os = "linux")]
    bwrap: Option<PathBuf>,
}

struct Shared {
    live: StdMutex<Arc<Live>>,
    network_options: Option<NetworkOptions>,
    tunnel_gate: Arc<TunnelGate>,
    monitor_violations: bool,
    policy: RwLock<RuntimePolicy>,
    callbacks: Callbacks,
    violations: ViolationStore,
    reasons: DenialReasons,
    active: StdMutex<HashMap<CommandId, ActiveCommand>>,
    network_lists: StdMutex<HashMap<String, CommandNetworkLists>>,
}

/// Runs commands under a filesystem and network policy, in this process,
/// with no message bus.
///
/// Every command gets its own egress proxy, served by a task here, so what
/// it is denied is recorded against that command alone.
#[derive(Clone)]
pub struct PolicyExec {
    shared: Arc<Shared>,
}

pub struct PolicyExecBuilder {
    config: PolicyExecConfig,
    callbacks: Callbacks,
    network_options: Option<NetworkOptions>,
    monitor_violations: bool,
    violations: Option<ViolationStore>,
}

impl PolicyExecBuilder {
    /// Answers for destinations the network policy does not decide. Without
    /// one, such a destination is denied.
    pub fn ask_callback(mut self, callback: impl AskCallback + 'static) -> Self {
        self.callbacks.ask = Some(Arc::new(callback));
        self
    }

    /// Decides each HTTP request to a destination the policy admitted.
    pub fn request_callback(mut self, callback: impl RequestCallback + 'static) -> Self {
        self.callbacks.request = Some(Arc::new(callback));
        self
    }

    /// Edits the headers of every HTTPS request just before it goes
    /// to the destination, after the request was allowed. It is given the destination
    /// host, never the client's `Host` header. A result that is not a valid
    /// header set, or that changes `Host`, `Content-Length` or
    /// `Transfer-Encoding`, is denied.
    pub fn header_mutator(mut self, mutator: impl HeaderMutator + 'static) -> Self {
        self.callbacks.header_mutator = Some(Arc::new(mutator));
        self
    }

    /// The same for plain-HTTP requests. Anything it sets travels in the
    /// clear.
    pub fn header_mutator_plaintext(mut self, mutator: impl HeaderMutator + 'static) -> Self {
        self.callbacks.plaintext_header_mutator = Some(Arc::new(mutator));
        self
    }

    /// Asks `transport` about each HTTP request to an admitted destination,
    /// together with the request callback when there is one. The first denial
    /// wins. The request body is shown to the decider on its request.
    pub fn request_decider(
        mut self,
        transport: Arc<dyn DeciderTransport>,
        limits: DeciderLimits,
        placements: Vec<CredentialPlacement>,
    ) -> Self {
        self.callbacks.request_decider = Some(RequestDeciderSetup {
            transport,
            limits,
            placements,
        });
        self
    }

    /// Optional proxy behaviour, each part off unless set: refusal reasons,
    /// a strict allowlist, a parent proxy, extra trusted CAs, a tunnel cap and
    /// stripped response headers.
    pub fn network_options(mut self, options: NetworkOptions) -> Self {
        self.network_options = Some(options);
        self
    }

    /// Whether the kernel's filesystem denials are recorded as violations.
    /// On unless turned off; proxy denials are
    /// recorded either way.
    pub fn violation_monitor(mut self, enabled: bool) -> Self {
        self.monitor_violations = enabled;
        self
    }

    /// Lets a request decision set headers on plain-HTTP requests too. Off,
    /// an allow that sets headers is refused there, since the request is not
    /// terminated and the edit would travel in the clear.
    pub fn plaintext_header_set(mut self, enabled: bool) -> Self {
        self.callbacks.plaintext_header_set = enabled;
        self
    }

    /// How long the ask callback has to answer and how many different
    /// destinations may be waiting on it at once. A destination over the
    /// limit is denied without being asked.
    pub fn ask_limits(mut self, limits: DeciderLimits) -> Self {
        self.callbacks.ask_limits = limits;
        self
    }

    /// Points tools that ignore the generic proxy variables (ssh through
    /// git, FTP clients, the Docker CLI, JVMs) at the proxy. A JVM only gets
    /// it when `java_agent_jar` is a file.
    pub fn tool_proxy_env(mut self, java_agent_jar: Option<PathBuf>) -> Self {
        self.callbacks.tools = ToolProxyEnv::enabled(java_agent_jar);
        self
    }

    /// [`Self::tool_proxy_env`] with the jar the settings name in
    /// `javaAgentJarPath`.
    pub fn tool_proxy_env_from(self, settings: &SandboxSettings) -> Self {
        self.tool_proxy_env(settings.java_agent_jar_path().map(Path::to_path_buf))
    }

    /// Records violations into `store`, which may be shared with other runtimes.
    pub fn violation_store(mut self, store: ViolationStore) -> Self {
        self.violations = Some(store);
        self
    }

    pub fn build(self) -> Result<PolicyExec, RuntimeError> {
        self.config.validate()?;
        #[cfg(target_os = "linux")]
        let bwrap = resolve_bwrap(&self.config)?;
        let tunnel_gate = Arc::new(TunnelGate::from_options(
            self.network_options
                .as_ref()
                .unwrap_or_else(|| self.config.configured_network_options()),
        ));
        Ok(PolicyExec {
            shared: Arc::new(Shared {
                policy: RwLock::new(self.config.policy().clone()),
                live: StdMutex::new(Arc::new(Live {
                    config: Arc::new(self.config),
                    #[cfg(target_os = "linux")]
                    bwrap,
                })),
                tunnel_gate,
                network_options: self.network_options,
                monitor_violations: self.monitor_violations,
                callbacks: self.callbacks,
                violations: self.violations.unwrap_or_default(),
                reasons: DenialReasons::new(),
                active: StdMutex::new(HashMap::new()),
                network_lists: StdMutex::new(HashMap::new()),
            }),
        })
    }
}

impl PolicyExec {
    pub fn builder(config: PolicyExecConfig) -> PolicyExecBuilder {
        PolicyExecBuilder {
            callbacks: Callbacks::default(),
            network_options: None,
            monitor_violations: true,
            config,
            violations: None,
        }
    }

    /// A builder that runs `settings` with every opt-in a standalone command
    /// wants, which is private ranges by name
    /// ([`SandboxSettings::to_standard_config`]) and the tool proxy variables.
    pub fn standard_builder(
        settings: &SandboxSettings,
        workspace: impl Into<PathBuf>,
    ) -> Result<PolicyExecBuilder, SettingsError> {
        let config = settings.to_standard_config(workspace)?;
        Ok(Self::builder(config).tool_proxy_env_from(settings))
    }

    pub fn new(config: PolicyExecConfig) -> Result<Self, RuntimeError> {
        Self::builder(config).build()
    }

    /// Whether this host can run the sandbox at all: Linux outside WSL1, or
    /// macOS.
    pub fn is_supported_platform() -> bool {
        crate::platform::is_supported()
    }

    /// Whether commands run under a backend that confines them. False for the
    /// `None` backend.
    pub fn is_sandboxing_enabled(&self) -> bool {
        !matches!(self.current().backend_kind(), IsolationBackendKind::None)
    }

    /// The configuration in force: what the runtime was built from, or the
    /// last one given to [`Self::update_config`], with the policy as it is
    /// now.
    pub async fn config(&self) -> PolicyExecConfig {
        let policy = self.policy().await;
        PolicyExecConfig::clone(&self.current()).with_policy(policy)
    }

    fn current(&self) -> Arc<PolicyExecConfig> {
        Arc::clone(&self.live().config)
    }

    fn live(&self) -> Arc<Live> {
        Arc::clone(&self.shared.live.lock().unwrap_or_else(|poisoned| poisoned.into_inner()))
    }

    fn callbacks(&self, config: &PolicyExecConfig) -> Callbacks {
        let network_options = self
            .shared
            .network_options
            .clone()
            .unwrap_or_else(|| config.configured_network_options().clone());
        Callbacks {
            plaintext_header_set: self.shared.callbacks.plaintext_header_set
                || network_options.plaintext_header_set.is_enabled(),
            network_options,
            tunnel_gate: Some(Arc::clone(&self.shared.tunnel_gate)),
            ..self.shared.callbacks.clone()
        }
    }

    pub fn violations(&self) -> &ViolationStore {
        &self.shared.violations
    }

    pub async fn policy(&self) -> RuntimePolicy {
        self.shared.policy.read().await.clone()
    }

    /// The text of `stderr` followed by the violations recorded for `command`
    /// in a `<sandbox_violations>` block; `stderr` unchanged when there are
    /// none.
    pub fn annotate_stderr(&self, command: &CommandId, stderr: &str) -> String {
        let violations = self.shared.violations.violations_for_command(command);
        if violations.is_empty() {
            return stderr.to_string();
        }
        let mut annotated = format!("{stderr}\n<sandbox_violations>\n");
        for violation in &violations {
            annotated.push_str(violation.line.as_str());
            annotated.push('\n');
        }
        annotated.push_str("</sandbox_violations>");
        annotated
    }

    /// Replaces the policy. Commands that start later get it, and so do
    /// commands that are running behind a proxy: their next connection is
    /// judged by the new network policy. `DenyAll` makes a running proxy
    /// refuse everything without asking the callback, and a command that
    /// starts under `DenyAll` is still routed through a proxy, so a later
    /// update can open its network. `AllowAll` makes it
    /// admit every destination that passes the proxy's address floor, which is
    /// narrower than the unproxied `AllowAll` that new commands get. The
    /// filesystem policy of a running command cannot change, and neither can
    /// its route: a command started under `AllowAll` keeps its direct
    /// egress. Connections already open are not interrupted. Nothing changes
    /// when the new policy is invalid.
    /// What a running command holds for its credential bindings: its sentinel
    /// registry, AWS pairs, masked files and the CA its proxy mints
    /// certificates with. `None` once the command has finished.
    pub fn command_credentials(&self, command: &CommandId) -> Option<CommandCredentials> {
        self.lock_active().get(command).map(|active| active.credentials.clone())
    }

    pub async fn update_policy(&self, policy: RuntimePolicy) -> Result<(), RuntimeError> {
        validate_policy(self.current().backend_kind(), &policy)?;
        let mut current = self.shared.policy.write().await;
        let handles: Vec<(PolicyHandle, Vec<DomainRule>)> = self
            .active()
            .into_iter()
            .filter(|active| active.follows_network)
            .filter_map(|active| active.policy.map(|handle| (handle, active.extra_rules)))
            .collect();
        for (handle, extra_rules) in handles {
            handle
                .replace(merge_command_rules(&policy.network, &extra_rules))
                .await
                .map_err(RuntimeError::PolicyRejectedError)?;
        }
        *current = policy;
        Ok(())
    }

    /// Replaces the whole configuration: the policy as
    /// [`Self::update_policy`] does, and the credential bindings, AWS pairs,
    /// SigV4 policies, ignored violations, proxy options (unless the builder
    /// set its own), platform opt-ins and `bwrap` path for the commands that
    /// start later. A running command keeps what it started with, except for the
    /// tunnel cap and handshake timeout, which every command reads at each
    /// CONNECT. The
    /// workspace, backend, tier, view, environment, limits and helper are
    /// fixed when the runtime is built and must be the same in `config`.
    /// Nothing changes when `config` is invalid or differs in a fixed part.
    pub async fn update_config(&self, config: PolicyExecConfig) -> Result<(), RuntimeError> {
        config.validate()?;
        let current = self.current();
        if let Some(part) = current.fixed_part_changed(&config) {
            return Err(RuntimeError::Invalid(anyhow::anyhow!(
                "{part} is fixed when the runtime is built and cannot be updated"
            )));
        }
        #[cfg(target_os = "linux")]
        let bwrap = resolve_bwrap(&config)?;
        self.update_policy(config.policy().clone()).await?;
        if self.shared.network_options.is_none() {
            let options = config.configured_network_options();
            self.shared.tunnel_gate.set_cap(options.tunnel_cap);
            self.shared.tunnel_gate.set_handshake(options.handshake_timeout);
        }
        *self.shared.live.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Arc::new(Live {
            config: Arc::new(config),
            #[cfg(target_os = "linux")]
            bwrap,
        });
        Ok(())
    }

    /// Sends `signal` to the sandbox process of every command that has
    /// started, and returns how many were signalled. A command whose backend
    /// launch is under way has no process id yet: the signal is held and
    /// delivered the moment the id is known, and the command counts as
    /// signalled. A command still being set up is not counted.
    pub fn signal_commands(&self, signal: Signal) -> usize {
        let mut signalled = 0;
        for active in self.lock_active().values_mut() {
            match active.pid {
                Some(pid) => {
                    if i32::try_from(pid).is_ok_and(|pid| nix::sys::signal::kill(Pid::from_raw(pid), signal).is_ok()) {
                        signalled += 1;
                    }
                }
                None if active.launching => {
                    active.pending_signals.push(signal);
                    signalled += 1;
                }
                None => {}
            }
        }
        signalled
    }

    /// Stops every running command and forgets the recorded violations. The
    /// runtime stays usable.
    pub async fn reset(&self) {
        for active in self.active() {
            active.cancel.cancel();
        }
        let waited = tokio::time::timeout(RESET_WAIT, async {
            while !self.lock_active().is_empty() {
                tokio::time::sleep(RESET_POLL).await;
            }
        })
        .await;
        if waited.is_err() {
            tracing::warn!("commands were still running when the reset gave up waiting");
        }
        self.lock_network_lists().clear();
        self.shared.violations.clear();
    }

    /// Gives the command named `id` extra allowed domains, on top of the
    /// ones in its `CommandSpec`. Registering an id again replaces its list,
    /// and the list takes effect on the command if it is already running.
    /// A strict allowlist ignores the lists, as it does `network_allow`.
    /// With reasoned refusals on and an external HTTP proxy port the lists
    /// never apply, because no connection names a command there.
    ///
    /// The error does not repeat the id.
    pub async fn register_command_network_lists(
        &self,
        id: &CommandId,
        lists: CommandNetworkLists,
    ) -> Result<(), RuntimeError> {
        let key = CommandAttribution::key_of(id.as_str());
        if key.chars().filter(|c| *c != '\u{FFFD}').count() < MIN_COMMAND_ID_LEN {
            return Err(RuntimeError::Invalid(anyhow::anyhow!(
                "command network lists: commandId must be a string of at least {MIN_COMMAND_ID_LEN} characters, unpaired surrogates not counted. The id is the only thing that binds a proxy connection to this allow list, and the sandboxed process chooses which id it presents, so it must be random (at least 128 bits, e.g. 16 random bytes as base64url) and never derived from the command text, a counter or the time."
            )));
        }
        if self
            .callbacks(&self.current())
            .network_options
            .external_proxy
            .is_some_and(|external| external.http_port().is_some())
        {
            tracing::debug!(
                "registering a command network list while an external HTTP proxy port is set: the list does not apply to what that proxy carries"
            );
        }
        self.lock_network_lists().insert(key, lists);
        self.refresh_commands_named_like(id).await
    }

    /// Takes back what [`Self::register_command_network_lists`] gave `id`.
    /// An id that was never registered is left alone.
    pub async fn unregister_command_network_lists(&self, id: &CommandId) -> Result<(), RuntimeError> {
        if self
            .lock_network_lists()
            .remove(&CommandAttribution::key_of(id.as_str()))
            .is_none()
        {
            return Ok(());
        }
        self.refresh_commands_named_like(id).await
    }

    /// Runs `spec` in the sandbox and waits for it to end. The proxy, scratch
    /// files and resource limits of the command are released before this
    /// returns.
    pub async fn run(&self, spec: CommandSpec) -> Result<CommandOutput, RuntimeError> {
        let mut started = self.start(&spec, false).await?;
        let cap = spec.output_limit.cap();
        let stdout = Capture::start(started.child.stdout.take(), cap);
        let stderr = Capture::start(started.child.stderr.take(), cap);

        let (status, timed_out) = supervise(&mut started.child, &started.cancel, spec.timeout).await;
        started.release_process().await;
        let (stdout, stdout_truncated) = stdout.finish().await;
        let (stderr, stderr_truncated) = stderr.finish().await;
        let command = started.command.clone();
        self.close(started).await;

        Ok(CommandOutput {
            command,
            status: status?,
            stdout,
            stderr,
            truncated: Truncation {
                stdout: stdout_truncated,
                stderr: stderr_truncated,
            },
            timed_out,
        })
    }

    /// Starts `spec` and returns while it runs. The pipes of the spec are
    /// handed to the caller instead of being captured, and a piped stdin
    /// stays open. The output limit does not apply.
    pub async fn spawn(&self, spec: CommandSpec) -> Result<RunningCommand, RuntimeError> {
        let mut started = self.start(&spec, true).await?;
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let running = RunningCommand {
            id: started.command.clone(),
            pid: started.child.id(),
            stdin: started.child.stdin.take(),
            stdout: started.child.stdout.take(),
            stderr: started.child.stderr.take(),
            cancel: started.cancel.clone(),
            exit: receiver,
        };
        let runtime = self.clone();
        let timeout = spec.timeout;
        tokio::spawn(async move {
            let (status, timed_out) = supervise(&mut started.child, &started.cancel, timeout).await;
            started.release_process().await;
            let command = started.command.clone();
            runtime.close(started).await;
            let _ = sender.send(
                status
                    .map(|status| CommandExit {
                        command,
                        status,
                        timed_out,
                    })
                    .map_err(RuntimeError::from),
            );
        });
        Ok(running)
    }

    async fn start(&self, spec: &CommandSpec, keep_stdin: bool) -> Result<Started, RuntimeError> {
        let live = self.live();
        let config = &live.config;
        let host_view = config.filesystem_view() == FilesystemView::Host;
        if spec.cwd.is_some() && !host_view {
            return Err(RuntimeError::unsupported(
                "a working directory per command needs the host view; the guest view starts in the workspace",
            ));
        }
        let command = match &spec.command_id {
            Some(id) => {
                if id.as_str().chars().count() < MIN_COMMAND_ID_LEN {
                    return Err(RuntimeError::Invalid(anyhow::anyhow!(
                        "a command id must have at least {MIN_COMMAND_ID_LEN} characters"
                    )));
                }
                id.clone()
            }
            None => CommandId::new(uuid::Uuid::new_v4().simple().to_string()),
        };
        let cancel = match &spec.cancel {
            Some(token) => token.child_token(),
            None => CancellationToken::new(),
        };
        let callbacks = self.callbacks(config);
        if cancel.is_cancelled() {
            return Err(RuntimeError::Cancelled);
        }

        let filter = ViolationFilter::new(config.ignore_violations_shared(), Some(spec.display_text()));
        let extra_rules = self.command_rules(&callbacks, &self.allowed_for(&callbacks, &command, &spec.network_allow));
        let policy_guard = self.shared.policy.read().await;
        let mut policy = policy_guard.clone();
        let own_credentials = match &spec.overrides {
            Some(overrides) => overrides.effective_credentials(config)?,
            None => None,
        };
        if spec.fs_policy.is_some() || spec.network_policy.is_some() || spec.overrides.is_some() {
            if let Some(overrides) = &spec.overrides {
                let session_deny = CredentialContext::of(config).map_or(&[][..], |context| context.session_deny());
                let credential_deny = own_credentials.as_ref().map_or(session_deny, |own| own.deny.as_slice());
                policy.fs =
                    Some(overrides.fs_with_credentials(&policy.fs.clone().unwrap_or_default(), credential_deny)?);
            }
            if let Some(fs) = &spec.fs_policy {
                policy.fs = Some(fs.clone());
            }
            if let Some(network) = &spec.network_policy {
                policy.network = network.clone();
            }
            validate_policy(config.backend_kind(), &policy)?;
        }
        let network = merge_command_rules(&policy.network, &extra_rules);
        let credentials = PreparedCredentials::prepare(config, own_credentials.as_ref(), &spec.env, &network).await?;
        if cancel.is_cancelled() {
            return Err(RuntimeError::Cancelled);
        }
        let wire_proxy = match &network {
            NetworkPolicy::UserDefined { .. } => true,
            NetworkPolicy::DenyAll => !callbacks
                .network_options
                .external_proxy
                .is_some_and(replaces_runtime_proxy),
            NetworkPolicy::AllowAll => false,
        };
        let proxy = if wire_proxy {
            Some(
                CommandProxy::start(
                    &network,
                    &command,
                    &callbacks,
                    &self.shared.violations,
                    &self.shared.reasons,
                    filter.clone(),
                    &credentials,
                )
                .await
                .map_err(RuntimeError::Proxy)?,
            )
        } else {
            None
        };
        if cancel.is_cancelled() {
            if let Some(proxy) = proxy {
                proxy.shutdown().await;
            }
            return Err(RuntimeError::Cancelled);
        }
        let duplicate = {
            let mut active = self.lock_active();
            if active.contains_key(&command) {
                true
            } else {
                active.insert(
                    command.clone(),
                    ActiveCommand {
                        cancel: cancel.clone(),
                        follows_network: spec.network_policy.is_none(),
                        policy: proxy.as_ref().and_then(CommandProxy::policy_handle),
                        pid: None,
                        launching: false,
                        pending_signals: Vec::new(),
                        spec_allow: spec.network_allow.clone(),
                        extra_rules,
                        credentials: credentials.snapshot().with_ca(
                            proxy
                                .as_ref()
                                .and_then(CommandProxy::ca_certificate_pem)
                                .map(str::to_string),
                        ),
                    },
                );
                false
            }
        };
        if duplicate {
            if let Some(proxy) = proxy {
                proxy.shutdown().await;
            }
            return Err(RuntimeError::Invalid(anyhow::anyhow!(
                "a command with this id is already running"
            )));
        }
        drop(policy_guard);

        match self
            .launch_command(
                &live,
                &command,
                spec,
                &policy,
                &network,
                proxy.as_ref(),
                &credentials,
                filter,
                keep_stdin,
            )
            .await
        {
            Ok(launched) => Ok(Started {
                command,
                cancel,
                child: launched.child,
                limits: Some(launched.limits),
                placeholders: launched.placeholders,
                monitors: launched.monitors,
                denials: launched.denials,
                proxy,
                _credentials: credentials,
            }),
            Err(err) => {
                if let Some(proxy) = proxy {
                    proxy.shutdown().await;
                }
                self.lock_active().remove(&command);
                Err(err)
            }
        }
    }

    fn allowed_for(&self, callbacks: &Callbacks, id: &CommandId, spec_allow: &[DomainPattern]) -> Vec<DomainPattern> {
        let mut allowed = spec_allow.to_vec();
        let options = &callbacks.network_options;
        let unreachable_by_id = options.reasoned_refusals.is_enabled()
            && options
                .external_proxy
                .as_ref()
                .is_some_and(|external| external.http_port().is_some());
        let key = CommandAttribution::key_of(id.as_str());
        if !unreachable_by_id
            && CommandAttribution::is_carried(id.as_str())
            && let Some(lists) = self.lock_network_lists().get(&key)
        {
            allowed.extend(lists.allowed_domains.iter().cloned());
        }
        allowed
    }

    async fn refresh_commands_named_like(&self, id: &CommandId) -> Result<(), RuntimeError> {
        let key = CommandAttribution::key_of(id.as_str());
        let running: Vec<CommandId> = self
            .lock_active()
            .keys()
            .filter(|running| CommandAttribution::key_of(running.as_str()) == key)
            .cloned()
            .collect();
        for running in &running {
            self.refresh_command_rules(running).await?;
        }
        Ok(())
    }

    async fn refresh_command_rules(&self, id: &CommandId) -> Result<(), RuntimeError> {
        let policy = self.shared.policy.read().await;
        let Some((handle, spec_allow)) = self
            .lock_active()
            .get(id)
            .and_then(|active| Some((active.policy.clone()?, active.spec_allow.clone())))
        else {
            return Ok(());
        };
        let callbacks = self.callbacks(&self.live().config);
        let rules = self.command_rules(&callbacks, &self.allowed_for(&callbacks, id, &spec_allow));
        handle
            .replace(merge_command_rules(&policy.network, &rules))
            .await
            .map_err(RuntimeError::PolicyRejectedError)?;
        if let Some(active) = self.lock_active().get_mut(id) {
            active.extra_rules = rules;
        }
        Ok(())
    }

    fn lock_network_lists(&self) -> std::sync::MutexGuard<'_, HashMap<String, CommandNetworkLists>> {
        self.shared
            .network_lists
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn command_rules(&self, callbacks: &Callbacks, allow: &[DomainPattern]) -> Vec<DomainRule> {
        if callbacks.network_options.unlisted == UnlistedDestinations::Refuse {
            return Vec::new();
        }
        allow
            .iter()
            .map(|domain| DomainRule {
                domain: domain.clone(),
                action: DomainRuleAction::PassThrough,
            })
            .collect()
    }

    async fn close(&self, started: Started) {
        drop(started.monitors);
        if let Some(denials) = started.denials {
            denials.finish().await;
        }
        if let Some(proxy) = started.proxy {
            proxy.shutdown().await;
        }
        self.lock_active().remove(&started.command);
    }

    #[allow(clippy::too_many_arguments)]
    async fn launch_command(
        &self,
        live: &Live,
        command: &CommandId,
        spec: &CommandSpec,
        policy: &RuntimePolicy,
        network: &NetworkPolicy,
        proxy: Option<&CommandProxy>,
        credentials: &PreparedCredentials,
        filter: ViolationFilter,
        keep_stdin: bool,
    ) -> Result<Launched, RuntimeError> {
        let config = &live.config;
        let host_view = config.filesystem_view() == FilesystemView::Host;
        let environment = proxy
            .map(CommandProxy::environment)
            .map(|environment| environment.with_attribution(&spec.attribution()));
        let mut proxy_vars = environment
            .as_ref()
            .map(|environment| environment.vars())
            .unwrap_or_default();
        if host_view {
            proxy_vars = host_env::with_host_ca(
                proxy_vars,
                CaCertificate::guest_path(),
                proxy.and_then(CommandProxy::ca_file),
            );
        }
        let platform = match &spec.overrides {
            Some(overrides) => overrides.platform_over(config.configured_platform_options()),
            None => config.configured_platform_options().clone(),
        };
        let filesystem_enforced = !platform.filesystem.disabled.is_disabled();
        let tmpdir = (host_view && filesystem_enforced).then(host_env::process_tmpdir);
        let tmpdir_layer: Vec<(String, String)> =
            tmpdir.iter().map(|dir| ("TMPDIR".to_string(), dir.clone())).collect();
        let mut env: Vec<(String, String)> = if host_view {
            host_env::merged(
                host_env::process_env(),
                &[&tmpdir_layer, config.environment(), spec.env.as_slice()],
            )
        } else {
            config.environment().iter().chain(spec.env.iter()).cloned().collect()
        };
        env.retain(|(name, _)| proxy_vars.iter().all(|(taken, _)| taken != name));
        let inherited_java = env
            .iter()
            .position(|(name, _)| name == JAVA_TOOL_OPTIONS)
            .map(|at| env.remove(at).1)
            .filter(|_| !credentials.withholds(JAVA_TOOL_OPTIONS));
        env.extend(proxy_vars);
        credentials.apply_to_env(&mut env);
        let local_binding = cfg!(target_os = "macos")
            && platform.macos.allow_local_binding.is_allowed()
            && !matches!(network, NetworkPolicy::AllowAll);
        let java_options = match &environment {
            Some(environment) => environment.java_tool_options(inherited_java.as_deref()),
            None if local_binding => java_tool_options(None, &[PREFER_IPV4_STACK], inherited_java.as_deref()),
            None => inherited_java,
        };
        if let Some(options) = java_options {
            env.push((JAVA_TOOL_OPTIONS.to_string(), options));
        }

        let egress_policy = match (network, proxy) {
            (_, Some(proxy)) => proxy.egress(config.helper_path()).map_err(RuntimeError::Proxy)?,
            (NetworkPolicy::AllowAll, None) => EgressPolicy::AllowAll,
            (_, None) => EgressPolicy::DenyAll,
        };
        let cwd = command_cwd(config, spec);
        let mut credential_fs = credentials.apply_to_fs_policy(policy.fs.clone())?;
        if credential_fs
            .as_ref()
            .is_some_and(|fs| !fs.credential_deny_read().is_empty())
        {
            let ca_path = if host_view {
                proxy
                    .and_then(CommandProxy::ca_file)
                    .map(|path| path.to_string_lossy().into_owned())
            } else {
                proxy
                    .and_then(CommandProxy::ca_certificate_pem)
                    .map(|_| CaCertificate::guest_path().to_string())
            };
            if let Some(own) = ca_path.and_then(|path| literal_entry(&path).ok()) {
                credential_fs = credential_fs
                    .map(|fs| {
                        FsAdditions {
                            allow_read_within_deny: vec![own.clone()],
                            own_allow_within_deny: vec![own],
                            ..FsAdditions::default()
                        }
                        .applied_to(&fs)
                    })
                    .transpose()?;
            }
        }
        if host_view && filesystem_enforced {
            let fs = credential_fs.unwrap_or_default();
            let defaults: Vec<FsPathEntry> = crate::restrictions::default_write_dir_entries(&fs).collect();
            credential_fs = Some(
                FsAdditions {
                    allow_write: defaults,
                    ..FsAdditions::default()
                }
                .applied_to(&fs)?,
            );
        }
        if host_view
            && let Some(store) = credentials
                .store_dir()
                .and_then(|dir| dir.canonicalize().ok())
                .and_then(|dir| literal_entry(&dir.to_string_lossy()).ok())
        {
            let fs = credential_fs.unwrap_or_default();
            credential_fs = Some(
                FsAdditions {
                    deny_write: vec![store],
                    ..FsAdditions::default()
                }
                .applied_to(&fs)?,
            );
        }
        let (workspace_sandbox_path, fs_policy, extra_binds) = if host_view {
            (
                cwd.clone(),
                Some(credential_fs.unwrap_or_default()),
                credentials.binds().to_vec(),
            )
        } else {
            (
                PathBuf::from(GUEST_WORKSPACE),
                credential_fs.filter(|fs| !fs.is_empty()),
                environment
                    .as_ref()
                    .and_then(|environment| environment.ca_bind())
                    .into_iter()
                    .chain(credentials.binds().iter().cloned())
                    .collect::<Vec<ExtraBind>>(),
            )
        };
        let launch = LaunchSpec {
            binary: spec.program.clone(),
            args: spec.args.clone(),
            env,
            workspace_host_path: if host_view {
                cwd.clone()
            } else {
                config.workspace_path().to_path_buf()
            },
            workspace_sandbox_path,
            workspace_readonly: false,
            egress_policy,
            extra_binds,
            user: None,
            limits: config.resource_limits(),
            fs_policy,
            platform,
            fs_denials: None,
        };

        let view = host_view.then(|| HostView::new(cwd));
        let placeholders = PlaceholderLedger::shared().begin();
        if let Some(active) = self.lock_active().get_mut(command) {
            active.launching = true;
        }
        let (launched, denials) = self.launch(live, &launch, spec.stdio, command, filter, view).await?;
        let LaunchedProcess {
            child,
            limits,
            placeholders: created,
            unguarded,
            monitors,
            ..
        } = launched;
        placeholders.record(&created);
        for path in &unguarded {
            tracing::warn!(
                command = %command,
                path = %path,
                "denied path does not exist and could not be guarded; the sandbox can create it"
            );
        }
        let taken = child.lock().await.take();
        let mut child =
            taken.ok_or_else(|| RuntimeError::Launch(anyhow::anyhow!("the backend returned no child process")))?;
        if !keep_stdin {
            drop(child.stdin.take());
        }
        if let Some(active) = self.lock_active().get_mut(command) {
            active.pid = child.id();
            if let Some(pid) = active.pid.and_then(|pid| i32::try_from(pid).ok()) {
                for signal in active.pending_signals.drain(..) {
                    let _ = nix::sys::signal::kill(Pid::from_raw(pid), signal);
                }
            }
        }
        Ok(Launched {
            child,
            limits,
            placeholders,
            monitors,
            denials,
        })
    }

    async fn launch_unconfined(spec: &LaunchSpec, stdio: ChildStdio) -> Result<LaunchedProcess, RuntimeError> {
        PassthroughBackend::default()
            .with_stdio(stdio)
            .launch(spec)
            .await
            .map_err(RuntimeError::launch)
    }

    #[cfg(target_os = "linux")]
    async fn launch(
        &self,
        live: &Live,
        spec: &LaunchSpec,
        stdio: ChildStdio,
        command: &CommandId,
        filter: ViolationFilter,
        view: Option<HostView>,
    ) -> Result<(LaunchedProcess, Option<KernelDenials>), RuntimeError> {
        let config = &live.config;
        let launched = match config.backend_kind() {
            IsolationBackendKind::None => Self::launch_unconfined(spec, stdio).await?,
            IsolationBackendKind::Bubblewrap => {
                let bwrap = live
                    .bwrap
                    .clone()
                    .ok_or_else(|| RuntimeError::Launch(anyhow::anyhow!("bubblewrap was not resolved")))?;
                let mut backend = BubblewrapBackend::new(bwrap).with_stdio(stdio);
                if let Some(view) = view {
                    backend = backend.with_host_view(view);
                }
                match config.helper_path() {
                    Some(helper) => {
                        backend = backend
                            .with_confiner(helper.to_path_buf())
                            .with_observer(helper.to_path_buf())
                            .with_guest_init(helper.to_path_buf());
                    }
                    None => {
                        if let Ok(executable) = std::env::current_exe() {
                            backend = backend.with_guest_init(executable);
                        }
                    }
                }
                if !self.shared.monitor_violations {
                    return backend
                        .launch(spec)
                        .await
                        .map(|launched| (launched, None))
                        .map_err(RuntimeError::launch);
                }
                let (sink, receiver) = FsDenialSink::channel();
                let denials = KernelDenials::record(receiver, self.shared.violations.clone(), command.clone(), filter);
                let observed = LaunchSpec {
                    fs_denials: Some(sink),
                    ..spec.clone()
                };
                match backend.launch(&observed).await {
                    Ok(launched) => return Ok((launched, Some(denials))),
                    Err(err) => {
                        denials.finish().await;
                        return Err(RuntimeError::launch(err));
                    }
                }
            }
        };
        Ok((launched, None))
    }

    #[cfg(target_os = "macos")]
    async fn launch(
        &self,
        live: &Live,
        spec: &LaunchSpec,
        stdio: ChildStdio,
        command: &CommandId,
        filter: ViolationFilter,
        view: Option<HostView>,
    ) -> Result<(LaunchedProcess, Option<KernelDenials>), RuntimeError> {
        match live.config.backend_kind() {
            IsolationBackendKind::None => Ok((Self::launch_unconfined(spec, stdio).await?, None)),
            IsolationBackendKind::Seatbelt if !self.shared.monitor_violations => {
                let mut backend = SeatbeltBackend::new().with_stdio(stdio);
                if let Some(view) = view {
                    backend = backend.with_host_view(view);
                }
                let launched = backend.launch(spec).await.map_err(RuntimeError::launch)?;
                Ok((launched, None))
            }
            IsolationBackendKind::Seatbelt => {
                let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
                let denials = KernelDenials::record(receiver, self.shared.violations.clone(), command.clone(), filter);
                let launched = {
                    let mut backend = SeatbeltBackend::new().with_stdio(stdio).with_violation_sink(sender);
                    if let Some(view) = view {
                        backend = backend.with_host_view(view);
                    }
                    backend.launch(spec).await.map_err(RuntimeError::launch)
                };
                match launched {
                    Ok(launched) => Ok((launched, Some(denials))),
                    Err(err) => {
                        denials.finish().await;
                        Err(err)
                    }
                }
            }
        }
    }

    fn lock_active(&self) -> std::sync::MutexGuard<'_, HashMap<CommandId, ActiveCommand>> {
        self.shared
            .active
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn active(&self) -> Vec<ActiveCommand> {
        self.lock_active()
            .values()
            .map(|active| ActiveCommand {
                cancel: active.cancel.clone(),
                policy: active.policy.clone(),
                pid: active.pid,
                launching: active.launching,
                pending_signals: active.pending_signals.clone(),
                spec_allow: active.spec_allow.clone(),
                extra_rules: active.extra_rules.clone(),
                follows_network: active.follows_network,
                credentials: active.credentials.clone(),
            })
            .collect()
    }
}

#[cfg(target_os = "linux")]
fn resolve_bwrap(config: &PolicyExecConfig) -> Result<Option<PathBuf>, RuntimeError> {
    if !matches!(config.backend_kind(), IsolationBackendKind::Bubblewrap) {
        return Ok(None);
    }
    match config.bwrap_path() {
        Some(path) => Ok(Some(path.to_path_buf())),
        None => which::which("bwrap").map(Some).map_err(|_| {
            RuntimeError::Invalid(anyhow::anyhow!(
                "bubblewrap (`bwrap`) not found on PATH; install it or set the bwrap binary"
            ))
        }),
    }
}

/// Collects what a child writes to one pipe through the shared head-and-tail
/// cap (the middle is replaced by a marker, as in every other worker), and
/// keeps reading past it so the child never blocks. Keeps what was read when
/// the pipe stays open after the child is gone.
struct Capture {
    kept: Arc<StdMutex<Option<CappedOutput>>>,
    reader: Option<tokio::task::JoinHandle<()>>,
}

impl Capture {
    fn start<R: AsyncRead + Unpin + Send + 'static>(pipe: Option<R>, cap: usize) -> Self {
        let kept = Arc::new(StdMutex::new(Some(CappedOutput::new(cap))));
        let reader = pipe.map(|mut pipe| {
            let kept = Arc::clone(&kept);
            tokio::spawn(async move {
                let mut chunk = [0u8; 8192];
                loop {
                    match pipe.read(&mut chunk).await {
                        Ok(0) | Err(_) => break,
                        Ok(read) => {
                            let mut kept = kept.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                            if let Some(output) = kept.as_mut() {
                                output.push(&chunk[..read]);
                            }
                        }
                    }
                }
            })
        });
        Self { kept, reader }
    }

    async fn finish(self) -> (Vec<u8>, bool) {
        if let Some(mut reader) = self.reader
            && tokio::time::timeout(PIPE_DRAIN_GRACE, &mut reader).await.is_err()
        {
            reader.abort();
        }
        let mut kept = self.kept.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        match kept.take() {
            Some(output) => {
                let truncated = output.truncation().is_truncated();
                (output.into_bytes(), truncated)
            }
            None => (Vec::new(), false),
        }
    }
}

#[cfg(test)]
mod tests;
