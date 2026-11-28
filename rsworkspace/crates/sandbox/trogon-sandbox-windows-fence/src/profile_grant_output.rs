use crate::profile_grant_item::items;
use crate::profile_grant_result::ProfileGrantResult;
use crate::profile_grant_wire::ProfileGrantWire;
use crate::windows_sandbox_error::WindowsSandboxError;
use crate::windows_sandbox_error_code::WindowsSandboxErrorCode as Code;

pub(crate) fn profile_grant_args(exclude: &[String], replace_exclusions: bool) -> Vec<String> {
    let mut args = vec!["profile".to_string(), "grant".to_string(), "--json".to_string()];
    for e in exclude {
        args.push("--exclude".to_string());
        args.push(e.clone());
    }
    if replace_exclusions {
        args.push("--replace-excludes".to_string());
    }
    args
}

pub(crate) fn parse_profile_grant(args: &[String], stdout: &str) -> Result<ProfileGrantResult, WindowsSandboxError> {
    let raw: ProfileGrantWire = serde_json::from_str(stdout).map_err(|e| {
        WindowsSandboxError::for_subcommand(
            Code::FenceHelperBadJson,
            format!(
                "fence-helper {}: unparseable JSON output {}: {e}",
                args.join(" "),
                serde_json::to_string(stdout).unwrap_or_default()
            ),
            "profile",
        )
    })?;
    Ok(ProfileGrantResult {
        profile: raw.profile,
        containers: raw.containers,
        granted: raw.granted,
        files: raw.files,
        denied: raw.denied,
        placeholders: raw.placeholders,
        writes: raw.writes,
        unchanged: raw.unchanged,
        removed: raw.removed,
        skipped: items("skipped", raw.skipped)?,
        not_granted: items("not_granted", raw.not_granted)?,
        self_closed: items("self_closed", raw.self_closed)?,
        unverifiable: items("unverifiable", raw.unverifiable)?,
        unprotected: items("unprotected", raw.unprotected)?,
        stale_allow: items("stale_allow", raw.stale_allow)?,
        forgotten_roots: items("forgotten_roots", raw.forgotten_roots)?,
        elapsed_ms: raw.elapsed_ms,
    })
}

#[cfg(test)]
mod tests;
