use crate::constants::WINDOWS_ALLOW_LOOPBACK_EFFECT;
use crate::install_command::install_command;
use crate::install_context::InstallContext;
use crate::install_instructions::install_instructions;
use crate::launch_error::seclogon_message;
use crate::sandbox_user_status::SandboxUserStatus;
use crate::seclogon_state::SeclogonState;
use crate::setup_version::setup_version_mismatch;
use crate::wfp_state::WfpState;
use crate::wfp_status::WfpStatus;
use crate::windows_sandbox_error::WindowsSandboxError;
use crate::windows_sandbox_error_code::WindowsSandboxErrorCode as Code;

/// The outcome of [`check_dependencies`]. Errors block a session; warnings
/// are informational.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DependencyCheck {
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
    pub reinstall_required: Option<String>,
    pub loopback_mismatch: Option<String>,
    codes: Vec<Code>,
}

impl DependencyCheck {
    /// The typed error that stops a session, if there is one: an install of
    /// another setup version and one of the other loopback mode have their
    /// own codes, any other problem is a plain failure.
    pub fn error(&self) -> Option<WindowsSandboxError> {
        if self.errors.is_empty() {
            return None;
        }
        if let Some(message) = &self.reinstall_required {
            return Some(WindowsSandboxError::new(Code::ReinstallRequired, message));
        }
        if let Some(message) = &self.loopback_mismatch {
            return Some(WindowsSandboxError::new(Code::WfpLoopbackMismatch, message));
        }
        let code = self.codes.first().copied().unwrap_or(Code::NotProvisioned);
        Some(WindowsSandboxError::new(
            code,
            format!("Sandbox dependencies not available: {}", self.errors.join(", ")),
        ))
    }
}

fn mode(open: bool) -> &'static str {
    if open { "open" } else { "closed" }
}

fn sublayer_name(ctx: &InstallContext) -> String {
    ctx.sublayer_guid
        .as_ref()
        .map_or_else(|| "(default)".to_string(), ToString::to_string)
}

/// The account, the Secondary Logon
/// service, the filters and their loopback mode against what `ctx` expects.
pub fn check_dependencies(user: &SandboxUserStatus, wfp: &WfpStatus, ctx: &InstallContext) -> DependencyCheck {
    let mut check = DependencyCheck::default();
    let provisioned = user.provisioned && user.cred_present;
    if !provisioned {
        check.codes.push(Code::NotProvisioned);
        check.errors.push(format!(
            "Sandbox user is not provisioned (user={}, cred={}). {}",
            user.provisioned,
            user.cred_present,
            install_instructions(ctx, false)
        ));
    } else if let Some(message) = setup_version_mismatch(user, ctx.sublayer_guid.as_ref()) {
        check.errors.push(message.clone());
        check.reinstall_required = Some(message);
    }

    if let Some(state @ (SeclogonState::Disabled | SeclogonState::Missing)) = user.seclogon {
        let name = if state == SeclogonState::Missing {
            "missing"
        } else {
            "disabled"
        };
        check.codes.push(Code::SeclogonDisabled);
        check.errors.push(seclogon_message(name).to_string());
    }

    match wfp.state {
        WfpState::CannotRead => {}
        WfpState::LoopbackMismatch => {
            check.codes.push(Code::WfpLoopbackMismatch);
            check.errors.push(format!(
                "WFP filters under sublayer {} cannot be used: their loopback mode ({}) is not the \
             one the install recorded. Re-run the install for this configuration (one UAC \
             prompt):\n  {}",
                sublayer_name(ctx),
                mode(wfp.allow_loopback),
                install_command(ctx, true)
            ));
        }
        WfpState::Absent => {
            if provisioned {
                check.codes.push(Code::NotProvisioned);
                check.errors.push(format!(
                    "WFP filters not installed under sublayer {}. {}",
                    sublayer_name(ctx),
                    install_instructions(ctx, false)
                ));
            }
        }
        WfpState::Installed => {}
    }

    let fenced = provisioned && matches!(wfp.state, WfpState::Installed | WfpState::CannotRead);
    if fenced && wfp.allow_loopback != ctx.allow_loopback {
        let message = format!(
            "WFP filters under sublayer {} do not match the configured loopback mode (installed: \
             loopback {}; configured: {}). Re-run the install for this configuration (one UAC \
             prompt):\n  {}",
            sublayer_name(ctx),
            mode(wfp.allow_loopback),
            mode(ctx.allow_loopback),
            install_command(ctx, true)
        );
        check.errors.push(message.clone());
        check.loopback_mismatch = Some(message);
    } else if fenced && wfp.allow_loopback {
        check.warnings.push(WINDOWS_ALLOW_LOOPBACK_EFFECT.to_string());
    }
    check
}
