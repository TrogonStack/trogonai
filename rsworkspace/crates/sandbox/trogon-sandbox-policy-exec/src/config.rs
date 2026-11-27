use std::any::Any;
use std::fmt::Debug;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use trogon_sandbox_contract::NetworkOptions;
use trogon_sandbox_contract::{AwsCredentialPairBinding, SecretBinding};
use trogon_sandbox_contract::{FsPolicy, MachServiceName, PlatformOptions};
use trogon_sandbox_contract::{IsolationTier, NetworkPolicy};

use crate::restrictions::{FsReadRestriction, FsWriteRestriction, NetworkRestriction};
use trogon_sandbox_egress_proxy::{IgnoreViolations, Sigv4Policies};
use trogon_sandbox_subprocess_worker::IsolationBackendKind;
use trogon_sandbox_subprocess_worker::isolation::resource_limits::ResourceLimits;

use crate::error::RuntimeError;

/// State a config built from settings carries for the settings, without the
/// config depending on the settings types.
pub(crate) trait SettingsContext: Any + Debug + Send + Sync {
    fn as_any(&self) -> &dyn Any;
}

impl<T: Any + Debug + Send + Sync> SettingsContext for T {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// What the filesystem looks like from inside the sandbox.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
pub enum FilesystemView {
    /// A guest filesystem built for the command: the workspace at
    /// `/workspace`, a private `/tmp`, and the policy in sandbox paths.
    #[default]
    Guest,
    /// The host filesystem, read-only except what the policy allows to be
    /// written. Policy paths are host paths, commands run in
    /// the directory they name, and the environment is inherited. An empty
    /// policy reads everything and writes nothing.
    Host,
}

/// What the sandbox allows, and what a running runtime can change in place.
/// Paths in the filesystem policy are sandbox paths: the workspace is at
/// `/workspace`.
#[derive(Debug, Clone)]
pub struct RuntimePolicy {
    pub fs: Option<FsPolicy>,
    pub network: NetworkPolicy,
}

impl Default for RuntimePolicy {
    fn default() -> Self {
        Self {
            fs: None,
            network: NetworkPolicy::DenyAll,
        }
    }
}

/// Everything a runtime is built from. The policy can be replaced later;
/// the rest is fixed.
#[derive(Debug, Clone)]
pub struct PolicyExecConfig {
    workspace: PathBuf,
    backend: IsolationBackendKind,
    policy: RuntimePolicy,
    isolation_tier: Option<IsolationTier>,
    secret_bindings: Vec<SecretBinding>,
    aws_pairs: Vec<AwsCredentialPairBinding>,
    sigv4: Sigv4Policies,
    plaintext_inject: bool,
    env: Vec<(String, String)>,
    limits: ResourceLimits,
    bwrap_binary: Option<PathBuf>,
    socat_binary: Option<PathBuf>,
    helper_binary: Option<PathBuf>,
    ignore_violations: Arc<IgnoreViolations>,
    network_options: NetworkOptions,
    platform: PlatformOptions,
    view: FilesystemView,
    credential_context: Option<Arc<dyn SettingsContext>>,
}

impl PolicyExecConfig {
    /// A runtime that runs commands in `workspace` on the platform's own
    /// isolation backend, with no network and no filesystem policy beyond
    /// the backend's default.
    pub fn new(workspace: impl Into<PathBuf>) -> Self {
        Self {
            workspace: workspace.into(),
            backend: native_backend(),
            policy: RuntimePolicy::default(),
            isolation_tier: None,
            secret_bindings: Vec::new(),
            aws_pairs: Vec::new(),
            sigv4: Sigv4Policies::default(),
            plaintext_inject: false,
            env: Vec::new(),
            limits: ResourceLimits::default(),
            bwrap_binary: None,
            socat_binary: None,
            helper_binary: None,
            ignore_violations: Arc::default(),
            network_options: NetworkOptions::default(),
            platform: PlatformOptions::default(),
            view: FilesystemView::default(),
            credential_context: None,
        }
    }

    pub fn backend(mut self, backend: IsolationBackendKind) -> Self {
        self.backend = backend;
        self
    }

    pub fn fs_policy(mut self, fs: FsPolicy) -> Self {
        self.policy.fs = Some(fs);
        self
    }

    pub fn network(mut self, network: NetworkPolicy) -> Self {
        self.policy.network = network;
        self
    }

    pub fn isolation_tier(mut self, tier: IsolationTier) -> Self {
        self.isolation_tier = Some(tier);
        self
    }

    pub fn secret_bindings(mut self, bindings: Vec<SecretBinding>) -> Self {
        self.secret_bindings = bindings;
        self
    }

    /// Links bindings whose variables do not carry the conventional AWS names
    /// into one credential, so a request signed with their sentinels is
    /// re-signed with the real values.
    pub fn aws_credential_pairs(mut self, pairs: Vec<AwsCredentialPairBinding>) -> Self {
        self.aws_pairs = pairs;
        self
    }

    /// What happens to a request signed with a masked AWS credential that
    /// cannot be re-signed. Every kind is denied unless set.
    pub fn sigv4_policies(mut self, policies: Sigv4Policies) -> Self {
        self.sigv4 = policies;
        self
    }

    /// Substitutes masked credentials in plain-HTTP requests too, and lets
    /// masking run without a TLS terminating network policy. The real value then travels in the
    /// clear, so it is off unless set.
    pub fn plaintext_inject(mut self, enabled: bool) -> Self {
        self.plaintext_inject = enabled;
        self
    }

    pub fn env(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.push((name.into(), value.into()));
        self
    }

    pub fn limits(mut self, limits: ResourceLimits) -> Self {
        self.limits = limits;
        self
    }

    /// Where `bwrap` is. Found on `PATH` when unset.
    pub fn bwrap_binary(mut self, path: impl Into<PathBuf>) -> Self {
        self.bwrap_binary = Some(path.into());
        self
    }

    /// Where `socat` is. This runtime relays in process, so it is only
    /// checked by `DependencyCheck`, which reports one that is not
    /// executable.
    pub fn socat_binary(mut self, path: impl Into<PathBuf>) -> Self {
        self.socat_binary = Some(path.into());
        self
    }

    /// The executable the Linux sandbox re-runs for its relay and confiner.
    /// It must call `run_helper_if_invoked` first thing in `main`.
    /// The running executable when unset, which then has to do that itself.
    pub fn helper_binary(mut self, path: impl Into<PathBuf>) -> Self {
        self.helper_binary = Some(path.into());
        self
    }

    /// Violations that are not recorded: lines that contain one of the
    /// patterns listed for the command (or for every command).
    pub fn ignore_violations(mut self, ignore: IgnoreViolations) -> Self {
        self.ignore_violations = Arc::new(ignore);
        self
    }

    /// Optional proxy behaviour, each part off unless set. A builder's own
    /// `network_options` takes precedence.
    pub fn network_options(mut self, options: NetworkOptions) -> Self {
        self.network_options = options;
        self
    }

    /// Opt-ins only some platforms can enforce. A backend that cannot enforce
    /// one refuses to launch the command.
    pub fn platform_options(mut self, platform: PlatformOptions) -> Self {
        self.platform = platform;
        self
    }

    /// Which filesystem the command sees. [`FilesystemView::Guest`] unless set.
    pub fn view(mut self, view: FilesystemView) -> Self {
        self.view = view;
        self
    }

    pub fn filesystem_view(&self) -> FilesystemView {
        self.view
    }

    pub fn workspace_path(&self) -> &Path {
        &self.workspace
    }

    pub fn backend_kind(&self) -> IsolationBackendKind {
        self.backend
    }

    pub fn policy(&self) -> &RuntimePolicy {
        &self.policy
    }

    pub fn tier(&self) -> Option<&IsolationTier> {
        self.isolation_tier.as_ref()
    }

    /// Keeps what the settings this config came from need to resolve a
    /// command's own credentials. The config only carries it; the settings
    /// read it back.
    pub(crate) fn credential_context(mut self, context: impl SettingsContext) -> Self {
        self.credential_context = Some(Arc::new(context));
        self
    }

    pub(crate) fn credential_context_any(&self) -> Option<&dyn Any> {
        self.credential_context.as_deref().map(SettingsContext::as_any)
    }

    pub fn bindings(&self) -> &[SecretBinding] {
        &self.secret_bindings
    }

    pub fn aws_pairs(&self) -> &[AwsCredentialPairBinding] {
        &self.aws_pairs
    }

    pub fn configured_sigv4_policies(&self) -> Sigv4Policies {
        self.sigv4
    }

    pub fn allows_plaintext_inject(&self) -> bool {
        self.plaintext_inject
    }

    pub fn environment(&self) -> &[(String, String)] {
        &self.env
    }

    pub fn resource_limits(&self) -> ResourceLimits {
        self.limits.clone()
    }

    pub fn bwrap_path(&self) -> Option<&Path> {
        self.bwrap_binary.as_deref()
    }

    pub fn socat_path(&self) -> Option<&Path> {
        self.socat_binary.as_deref()
    }

    pub fn helper_path(&self) -> Option<&Path> {
        self.helper_binary.as_deref()
    }

    pub fn configured_network_options(&self) -> &NetworkOptions {
        &self.network_options
    }

    pub fn configured_platform_options(&self) -> &PlatformOptions {
        &self.platform
    }

    pub fn ignored_violations(&self) -> &IgnoreViolations {
        &self.ignore_violations
    }

    #[cfg_attr(windows, allow(dead_code))]
    pub(crate) fn with_policy(mut self, policy: RuntimePolicy) -> Self {
        self.policy = policy;
        self
    }

    /// The first part that cannot change after the runtime is built and
    /// differs from `other`.
    #[cfg_attr(windows, allow(dead_code))]
    pub(crate) fn fixed_part_changed(&self, other: &Self) -> Option<&'static str> {
        let parts = [
            ("the workspace", self.workspace == other.workspace),
            ("the backend", self.backend == other.backend),
            ("the isolation tier", self.isolation_tier == other.isolation_tier),
            ("the filesystem view", self.view == other.view),
            ("the environment", self.env == other.env),
            ("the resource limits", self.limits == other.limits),
            ("the helper binary", self.helper_binary == other.helper_binary),
        ];
        parts.into_iter().find_map(|(name, same)| (!same).then_some(name))
    }

    /// The read restriction the policy implies, if any.
    pub fn fs_read_restriction(&self) -> Option<FsReadRestriction> {
        if self.platform.filesystem.disabled.is_disabled() {
            return Some(FsReadRestriction::default());
        }
        self.policy.fs.as_ref().map(FsReadRestriction::of)
    }

    /// The write restriction the policy implies, if any.
    pub fn fs_write_restriction(&self) -> Option<FsWriteRestriction> {
        if self.platform.filesystem.disabled.is_disabled() {
            return Some(FsWriteRestriction::everywhere());
        }
        self.policy.fs.as_ref().map(FsWriteRestriction::of)
    }

    /// The network restriction the policy implies.
    pub fn network_restriction(&self) -> NetworkRestriction {
        NetworkRestriction::of(&self.policy.network)
    }

    /// Whether the payload may bind local ports.
    pub fn allow_local_binding(&self) -> bool {
        self.platform.macos.allow_local_binding.is_allowed()
    }

    /// The Mach services the payload may look up.
    pub fn allow_mach_lookup(&self) -> &[MachServiceName] {
        &self.platform.macos.allow_mach_lookup
    }

    /// Whether the weaker nested sandbox mode is on.
    pub fn enable_weaker_nested_sandbox(&self) -> bool {
        self.platform.linux.weaker_nested_sandbox.is_weaker()
    }

    #[cfg_attr(windows, allow(dead_code))]
    pub(crate) fn ignore_violations_shared(&self) -> Arc<IgnoreViolations> {
        Arc::clone(&self.ignore_violations)
    }

    #[cfg_attr(windows, allow(dead_code))]
    pub(crate) fn validate(&self) -> Result<(), RuntimeError> {
        #[cfg(unix)]
        {
            trogon_sandbox_subprocess_worker::subprocess::ensure_tier_fits_backend(
                self.backend,
                self.isolation_tier.as_ref(),
            )
            .map_err(RuntimeError::Invalid)?;
            if !self.secret_bindings.is_empty() {
                trogon_sandbox_subprocess_worker::subprocess::ensure_secrets_fit_backend(self.backend)
                    .map_err(RuntimeError::Invalid)?;
            }
        }
        if !self.workspace.is_dir() {
            return Err(RuntimeError::Invalid(anyhow::anyhow!(
                "workspace {} is not a directory",
                self.workspace.display()
            )));
        }
        validate_policy(self.backend, &self.policy)?;
        if self.view == FilesystemView::Host && !self.workspace.is_absolute() {
            return Err(RuntimeError::Invalid(anyhow::anyhow!(
                "the host view needs an absolute working directory, got {}",
                self.workspace.display()
            )));
        }
        Ok(())
    }
}

#[cfg_attr(windows, allow(dead_code))]
pub(crate) fn validate_policy(backend: IsolationBackendKind, policy: &RuntimePolicy) -> Result<(), RuntimeError> {
    if matches!(backend, IsolationBackendKind::None) {
        if policy.fs.as_ref().is_some_and(|fs| !fs.is_empty()) {
            return Err(RuntimeError::Invalid(anyhow::anyhow!(
                "isolation backend `none` cannot enforce an fs policy"
            )));
        }
        if !matches!(policy.network, NetworkPolicy::AllowAll) {
            return Err(RuntimeError::Invalid(anyhow::anyhow!(
                "isolation backend `none` cannot enforce a network policy, so the network must be AllowAll"
            )));
        }
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn native_backend() -> IsolationBackendKind {
    IsolationBackendKind::Bubblewrap
}

#[cfg(target_os = "macos")]
fn native_backend() -> IsolationBackendKind {
    IsolationBackendKind::Seatbelt
}

#[cfg(windows)]
fn native_backend() -> IsolationBackendKind {
    IsolationBackendKind::Fenced
}
