use crate::install_context::InstallContext;
use crate::sublayer_guid::SublayerGuid;
use std::path::Path;
use std::time::Duration;

use crate::constants::DEFAULT_PROXY_PORT_RANGE;
use crate::constants::INSTALL_TIMEOUT_MS;
use crate::constants::PROFILE_GRANT_TIMEOUT_MS;
use crate::exit_code::ExitCode;
use crate::fence_helper::FenceHelper;
use crate::install_args::install_args;
use crate::install_args::uninstall_args;
use crate::install_failure::check_install_exit;
use crate::install_failure::profile_grant_failure;
use crate::install_failure::remap_install_timeout;
use crate::install_options::InstallOptions;
use crate::install_outcome::InstallOutcome;
use crate::install_profile_grant::InstallProfileGrant;
use crate::install_progress::InstallProgress;
use crate::interpret_wfp_verify::interpret_wfp_verify;
use crate::launch_error::parse_launch_error;
use crate::probe_listener::ProbeListener;
use crate::profile_grant_notice::profile_grant_notice;
use crate::profile_grant_output::parse_profile_grant;
use crate::profile_grant_output::profile_grant_args;
use crate::profile_grant_result::ProfileGrantResult;
use crate::profile_grant_skip_reason::profile_grant_skip_reason;
use crate::sandbox_status::SandboxStatus;
use crate::sandbox_status_parse::parse_sandbox_status;
use crate::uninstall_options::UninstallOptions;
use crate::uninstall_outcome::UninstallOutcome;
use crate::verify_options::VerifyOptions;
use crate::verify_run::VerifyRun;
use crate::wfp_verify_result::WfpVerifyResult;
use crate::windows_sandbox_error::WindowsSandboxError;
use crate::windows_sandbox_error_code::WindowsSandboxErrorCode as Code;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(15);

const VERIFY_TIMEOUT: Duration = Duration::from_secs(30);

const TRUST_CA_TIMEOUT: Duration = Duration::from_secs(60);

fn millis(ms: u64) -> Duration {
    Duration::from_millis(ms)
}

fn home_display() -> String {
    ["USERPROFILE", "HOME"]
        .iter()
        .find_map(|k| std::env::var(k).ok().filter(|v| !v.is_empty()))
        .unwrap_or_else(|| "your user profile".to_string())
}

/// The host side of the Windows network fence, driven through `fence-helper`.
#[derive(Debug, Clone)]
pub struct WindowsFence {
    fence_helper: FenceHelper,
}

impl WindowsFence {
    pub fn new(fence_helper: FenceHelper) -> Self {
        Self { fence_helper }
    }

    pub fn fence_helper(&self) -> &FenceHelper {
        &self.fence_helper
    }

    /// Sandbox-user provisioning state and the WFP filter set in one
    /// `fence-helper status` spawn. Needs no elevation; WFP degrades to
    /// `cannot-read` for a non-elevated caller.
    pub fn status(&self, sublayer_guid: Option<&SublayerGuid>) -> Result<SandboxStatus, WindowsSandboxError> {
        let mut args = vec!["status".to_string()];
        if let Some(guid) = sublayer_guid {
            args.extend(["--sublayer-guid".to_string(), guid.to_string()]);
        }
        let stdout = self.fence_helper.run_json(&args, DEFAULT_TIMEOUT)?;
        parse_sandbox_status(&stdout, &args)
    }

    /// One-shot install: provisions the sandbox account and the user-keyed
    /// WFP filter set in a single self-elevating process (one UAC prompt),
    /// then applies the profile read grant. Idempotent. A dismissed UAC
    /// prompt is [`InstallOutcome::Cancelled`], not an error.
    pub fn install(
        &self,
        opts: &InstallOptions,
        progress: &mut dyn InstallProgress,
    ) -> Result<InstallOutcome, WindowsSandboxError> {
        let args = install_args(opts);
        let timeout = millis(opts.timeout_ms.unwrap_or(INSTALL_TIMEOUT_MS));
        let out = self.fence_helper.run(&args, timeout).map_err(remap_install_timeout)?;
        check_install_exit(out.status, &out.stdout, &out.stderr)?;
        let status = self.status(opts.sublayer_guid.as_ref())?;
        if ExitCode::UAC_CANCELLED.matches(out.status) {
            return Ok(InstallOutcome::Cancelled);
        }
        if let Some(reason) = profile_grant_skip_reason(&status.user, opts.skip_profile_grant) {
            return Ok(InstallOutcome::Installed {
                status,
                profile_grant: InstallProfileGrant::Skipped { reason },
            });
        }
        progress.profile_grant_start(&profile_grant_notice(&home_display()));
        let result = self
            .profile_grant(
                &opts.profile_exclude,
                false,
                opts.profile_grant_timeout_ms,
                &mut |line| progress.profile_grant_line(line),
            )
            .map_err(profile_grant_failure)?;
        let user_sid = status.user.real_user_sid.clone();
        Ok(InstallOutcome::Installed {
            status,
            profile_grant: InstallProfileGrant::Applied { user_sid, result },
        })
    }

    /// Remove the filter set and the sandbox account, its credential and the
    /// setup marker (one UAC prompt). Idempotent.
    pub fn uninstall(&self, opts: &UninstallOptions) -> Result<UninstallOutcome, WindowsSandboxError> {
        let args = uninstall_args(opts);
        let default_ms = if opts.keep_user {
            INSTALL_TIMEOUT_MS
        } else {
            INSTALL_TIMEOUT_MS + PROFILE_GRANT_TIMEOUT_MS
        };
        let out = self
            .fence_helper
            .run(&args, millis(opts.timeout_ms.unwrap_or(default_ms)))?;
        if ExitCode::UAC_CANCELLED.matches(out.status) {
            return Ok(UninstallOutcome::Cancelled);
        }
        if out.status != Some(0) {
            return Err(WindowsSandboxError::new(
                Code::UninstallFailed,
                format!(
                    "fence-helper uninstall failed (exit {}): {}",
                    out.status.map_or_else(|| "none".to_string(), |s| s.to_string()),
                    if out.stderr.is_empty() {
                        &out.stdout
                    } else {
                        &out.stderr
                    }
                ),
            ));
        }
        Ok(UninstallOutcome::Removed)
    }

    /// Write or refresh the profile read grant, as this user, unelevated.
    /// Runs only while no sandbox session or command is running.
    pub fn profile_grant(
        &self,
        exclude: &[String],
        replace_exclusions: bool,
        timeout_ms: Option<u64>,
        on_line: &mut dyn FnMut(&str),
    ) -> Result<ProfileGrantResult, WindowsSandboxError> {
        let args = profile_grant_args(exclude, replace_exclusions);
        let ms = timeout_ms.unwrap_or(PROFILE_GRANT_TIMEOUT_MS);
        let out = self
            .fence_helper
            .run_streaming(&args, millis(ms), on_line)
            .map_err(|e| {
                if e.code() == Code::FenceHelperTimeout {
                    WindowsSandboxError::for_subcommand(
                        Code::FenceHelperTimeout,
                        format!(
                            "fence-helper profile grant timed out after {ms}ms; the grant is recorded \
                             as incomplete - run it again, or remove it with `fence-helper profile \
                             forget`"
                        ),
                        "profile",
                    )
                } else {
                    e
                }
            })?;
        if out.status != Some(0) {
            let tail: String = {
                let chars: Vec<char> = out.stderr.chars().collect();
                chars[chars.len().saturating_sub(2000)..].iter().collect()
            };
            return Err(WindowsSandboxError::for_subcommand(
                Code::ProfileGrantFailed,
                format!(
                    "fence-helper profile grant exited {}: {}",
                    out.status.map_or_else(|| "null".to_string(), |s| s.to_string()),
                    if tail.is_empty() { &out.stdout } else { &tail }
                ),
                "profile",
            ));
        }
        parse_profile_grant(&args, &out.stdout)
    }

    /// Remove the profile read grant.
    pub fn profile_forget(&self, timeout_ms: Option<u64>) -> Result<(), WindowsSandboxError> {
        let args = ["profile", "forget", "--json"].map(String::from);
        let out = self
            .fence_helper
            .run(&args, millis(timeout_ms.unwrap_or(PROFILE_GRANT_TIMEOUT_MS)))?;
        if out.status != Some(0) {
            return Err(WindowsSandboxError::for_subcommand(
                Code::ProfileForgetFailed,
                format!(
                    "The profile read grant could not be removed (fence-helper profile forget exited \
                     {}): {}",
                    out.status.map_or_else(|| "null".to_string(), |s| s.to_string()),
                    if out.stderr.is_empty() {
                        &out.stdout
                    } else {
                        &out.stderr
                    }
                ),
                "profile",
            ));
        }
        Ok(())
    }

    /// Installs (or replaces) a certificate authority in the sandbox
    /// account's `CurrentUser\Root` store and records it, which `fence-helper user
    /// trust-ca` does without elevation. The first call may create the
    /// account's profile.
    pub fn trust_ca(&self, cert_path: &Path) -> Result<(), WindowsSandboxError> {
        let args = vec![
            "user".to_string(),
            "trust-ca".to_string(),
            cert_path.display().to_string(),
        ];
        let out = self.fence_helper.run(&args, TRUST_CA_TIMEOUT)?;
        if ExitCode::SECLOGON_DISABLED.matches(out.status)
            && let Some(e) = parse_launch_error(&out.stderr, "user")
            && e.code() == Code::SeclogonDisabled
        {
            return Err(e);
        }
        if out.status != Some(0) {
            return Err(WindowsSandboxError::new(
                Code::TrustCaFailed,
                format!(
                    "fence-helper user trust-ca '{}' failed (exit {}): {}",
                    cert_path.display(),
                    out.status.map_or_else(|| "null".to_string(), |s| s.to_string()),
                    if out.stderr.is_empty() {
                        &out.stdout
                    } else {
                        &out.stderr
                    }
                ),
            ));
        }
        Ok(())
    }

    /// Behavioural proof that the WFP egress fence is active for the sandbox
    /// account: binds a local listener outside the permit range, has
    /// `fence-helper` connect to it as the sandbox account under each token shape
    /// a sandboxed process can present, and reads the outcome. Needs no
    /// elevation and no external host.
    pub fn verify(&self, opts: &VerifyOptions) -> Result<WfpVerifyResult, WindowsSandboxError> {
        let range = opts.proxy_port_range.unwrap_or(DEFAULT_PROXY_PORT_RANGE);
        let listener;
        let target = match &opts.target {
            Some(t) => t.clone(),
            None => {
                listener = ProbeListener::bind_outside(range)?;
                listener.addr.to_string()
            }
        };
        let mut args = vec!["wfp".to_string(), "verify".to_string(), "--target".to_string(), target];
        if opts.anonymous_egress {
            args.push("--anonymous-egress".to_string());
        }
        if opts.allow_loopback {
            args.push("--allow-loopback".to_string());
        }
        args.push("--write-restricted-token".to_string());
        for r in &opts.write_roots {
            args.extend(["--write-root".to_string(), r.clone()]);
        }
        let out = self.fence_helper.run(&args, VERIFY_TIMEOUT)?;
        if ExitCode::SECLOGON_DISABLED.matches(out.status)
            && let Some(e) = parse_launch_error(&out.stderr, "wfp")
            && e.code() == Code::SeclogonDisabled
        {
            return Err(e);
        }
        interpret_wfp_verify(
            &VerifyRun {
                status: out.status,
                stdout: out.stdout,
                stderr: out.stderr,
            },
            &InstallContext {
                sublayer_guid: opts.sublayer_guid.clone(),
                proxy_port_range: opts.proxy_port_range,
                sandbox_user: opts.sandbox_user.clone(),
                allow_loopback: opts.allow_loopback,
            },
        )
    }
}
