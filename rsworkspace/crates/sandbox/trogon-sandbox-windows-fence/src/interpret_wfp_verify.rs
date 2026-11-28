use crate::constants::WINDOWS_ALLOW_LOOPBACK_EFFECT;
use crate::exit_code::ExitCode;
use crate::install_command::install_command;
use crate::install_context::InstallContext;
use crate::probe_outcome::ProbeOutcome;
use crate::verify_run::VerifyRun;
use crate::wfp_residual::WfpResidual;
use crate::wfp_verify_probes::WfpVerifyProbes;
use crate::wfp_verify_result::WfpVerifyResult;
use crate::wfp_verify_wire::WfpVerifyWire;
use crate::windows_sandbox_error::WindowsSandboxError;
use crate::windows_sandbox_error_code::WindowsSandboxErrorCode as Code;

fn shown_status(status: Option<i32>) -> String {
    status.map_or_else(|| "without an exit code".to_string(), |s| s.to_string())
}

/// Turn one `fence-helper wfp verify` run into a result or a typed error (pure).
/// An attempt the fence must refuse that got through rejects whatever the
/// exit code says; a residual never throws.
pub fn interpret_wfp_verify(run: &VerifyRun, install: &InstallContext) -> Result<WfpVerifyResult, WindowsSandboxError> {
    let Ok(raw) = serde_json::from_str::<WfpVerifyWire>(&run.stdout) else {
        if run.stderr.contains("unexpected argument '--allow-loopback'") {
            return Err(crate::install_failure::fence_helper_too_old(&run.stderr));
        }
        return Err(WindowsSandboxError::new(
            Code::WfpVerifyUnparseable,
            format!(
                "WFP egress fence could not be verified - `fence-helper wfp verify` exited {} with \
                 unparseable output {} (stderr: {})",
                shown_status(run.status),
                serde_json::to_string(&run.stdout).unwrap_or_default(),
                serde_json::to_string(&run.stderr).unwrap_or_default()
            ),
        ));
    };
    let bad = |why: String| {
        WindowsSandboxError::for_subcommand(
            Code::FenceHelperBadJson,
            format!("fence-helper wfp verify: {why}"),
            "wfp",
        )
    };
    let Some(allow_loopback) = raw.allow_loopback else {
        return Err(bad("the report does not say `allow_loopback`".to_string()));
    };
    let session = InstallContext {
        allow_loopback,
        ..install.clone()
    };
    if ExitCode::LOOPBACK_MISMATCH.matches(run.status) || raw.allow_loopback_mismatch == Some(true) {
        return Err(WindowsSandboxError::new(
            Code::WfpLoopbackMismatch,
            format!(
                "WFP egress fence does not match the configured loopback mode - {}. Through this \
                 package: `{}`.",
                raw.hint
                    .as_deref()
                    .unwrap_or("the installed filter set has a different loopback mode"),
                install_command(&session, true)
            ),
        ));
    }
    let outcome = |name: &str, v: &Option<String>| match v.as_deref() {
        None => Err(bad(format!("the report has no `{name}`"))),
        Some(text) => ProbeOutcome::parse(text)
            .ok_or_else(|| bad(format!("`{name}` is {text:?}, an outcome this release does not know"))),
    };
    let probes = WfpVerifyProbes {
        plain: outcome("egress_probe", &raw.egress_probe)?,
        anonymous: outcome("anonymous_probe", &raw.anonymous_probe)?,
        anonymous_loopback: outcome("anonymous_loopback_probe", &raw.anonymous_loopback_probe)?,
        deny_only: outcome("deny_only_probe", &raw.deny_only_probe)?,
        low_integrity: outcome("low_integrity_probe", &raw.low_integrity_probe)?,
        low_integrity_loopback: outcome("low_integrity_loopback_probe", &raw.low_integrity_loopback_probe)?,
        untrusted_integrity: outcome("untrusted_integrity_probe", &raw.untrusted_integrity_probe)?,
        untrusted_integrity_loopback: outcome(
            "untrusted_integrity_loopback_probe",
            &raw.untrusted_integrity_loopback_probe,
        )?,
        open_loopback: if allow_loopback {
            Some(outcome("open_loopback_probe", &raw.open_loopback_probe)?)
        } else {
            None
        },
    };
    let off_loopback = raw.anonymous_target.as_deref().unwrap_or("no target");
    let blocked_target = raw.egress_target.as_deref().unwrap_or(&raw.target);
    let mut summary = format!(
        "plain={} ({blocked_target}), anonymous={} ({off_loopback}), anonymous-loopback={}, \
         deny-only={}, low-integrity={} ({off_loopback}), low-integrity-loopback={}, \
         untrusted-integrity={} ({off_loopback}), untrusted-integrity-loopback={}",
        probes.plain,
        probes.anonymous,
        probes.anonymous_loopback,
        probes.deny_only,
        probes.low_integrity,
        probes.low_integrity_loopback,
        probes.untrusted_integrity,
        probes.untrusted_integrity_loopback,
    );
    if let Some(open) = &probes.open_loopback {
        summary.push_str(&format!(", open-loopback={open} ({})", raw.target));
    }
    let low_il = "the sandbox user under a low-integrity copy of its token";
    let untrusted_il = "the sandbox user under an untrusted-integrity copy of its token";
    let anonymous_target = raw.anonymous_target.as_deref().unwrap_or("undefined");
    let mut which: Vec<String> = [
        (&probes.plain, "the sandbox user", blocked_target),
        (&probes.low_integrity, low_il, anonymous_target),
        (&probes.low_integrity_loopback, low_il, blocked_target),
        (&probes.untrusted_integrity, untrusted_il, anonymous_target),
        (&probes.untrusted_integrity_loopback, untrusted_il, blocked_target),
    ]
    .into_iter()
    .filter(|(o, _, _)| o.got_past_the_filters())
    .map(|(_, who, tgt)| format!("direct outbound from {who} to {tgt}"))
    .collect();
    if allow_loopback && probes.plain == ProbeOutcome::Unreachable {
        which.push(format!(
            "direct outbound from the sandbox user to {blocked_target} (no answer where the \
             filter must refuse at once)"
        ));
    }
    if ExitCode::FENCE_INACTIVE.matches(run.status) || !which.is_empty() {
        let what = if which.is_empty() {
            "a direct outbound connect".to_string()
        } else {
            which.join(" and ")
        };
        return Err(WindowsSandboxError::new(
            Code::WfpFenceInactive,
            format!(
                "WFP egress fence is not active - {what} was not blocked [{summary}]. Re-run `{}` \
                 (one UAC prompt). ({})",
                install_command(&session, false),
                run.stderr
            ),
        ));
    }
    if run.status != Some(0) {
        return Err(WindowsSandboxError::new(
            Code::WfpVerifyInconclusive,
            format!(
                "WFP egress fence could not be verified - probe to {blocked_target} was '{}' \
                 (exit {}) [{summary}]. The fence may be absent. Re-run `fence-helper install`. ({})",
                raw.egress_probe.as_deref().unwrap_or("undefined"),
                shown_status(run.status),
                run.stderr
            ),
        ));
    }
    if raw.egress_probe.as_deref() != Some("blocked") {
        return Err(WindowsSandboxError::new(
            Code::WfpVerifyInconclusive,
            format!(
                "WFP egress fence could not be verified - `fence-helper wfp verify` exited 0 but \
                 reported the plain attempt as '{}' [{summary}]. ({})",
                raw.egress_probe.as_deref().unwrap_or("missing"),
                run.stderr
            ),
        ));
    }
    let reported = raw.residuals.unwrap_or_default();
    let residuals: Vec<WfpResidual> = reported.iter().filter_map(|r| WfpResidual::parse(r)).collect();
    let mut warnings: Vec<String> = residuals
        .iter()
        .map(|r| format!("WFP egress fence residual '{}': {}", r.as_str(), r.text()))
        .collect();
    warnings.extend(
        reported
            .iter()
            .filter(|r| WfpResidual::parse(r).is_none())
            .map(|r| format!("WFP egress fence residual '{r}' is not one this release knows; treat it as a gap")),
    );
    if probes.low_integrity == ProbeOutcome::Skipped || probes.untrusted_integrity == ProbeOutcome::Skipped {
        warnings.push(
            "WFP egress fence: the off-loopback lowered-integrity attempts were skipped (no \
             off-loopback address on this machine, or an fence-helper that does not make them)"
                .to_string(),
        );
    }
    if allow_loopback {
        warnings.push(WINDOWS_ALLOW_LOOPBACK_EFFECT.to_string());
    }
    Ok(WfpVerifyResult {
        target: raw.target,
        stderr: run.stderr.clone(),
        probes,
        residuals,
        warnings,
    })
}
