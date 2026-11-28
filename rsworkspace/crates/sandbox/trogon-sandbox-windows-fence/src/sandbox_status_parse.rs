use crate::grant_state::GrantState;
use crate::persistent_grants::PersistentGrants;
use crate::profile_grant_record::ProfileGrantRecord;
use crate::proxy_port_range::ProxyPortRange;
use crate::sandbox_status::SandboxStatus;
use crate::sandbox_status_wire::SandboxStatusWire;
use crate::sandbox_status_wire::UserStatusWire;
use crate::sandbox_status_wire::WfpStatusWire;
use crate::sandbox_user_status::SandboxUserStatus;
use crate::seclogon_state::SeclogonState;
use crate::wfp_status::WfpStatus;
use crate::windows_sandbox_error::WindowsSandboxError;
use crate::windows_sandbox_error_code::WindowsSandboxErrorCode as Code;

fn non_empty(s: Option<String>) -> Option<String> {
    s.filter(|s| !s.is_empty())
}

fn grant_state(s: &str) -> GrantState {
    if s == "present" {
        GrantState::Present
    } else {
        GrantState::Unknown
    }
}

fn map_wfp(raw: WfpStatusWire) -> Result<WfpStatus, WindowsSandboxError> {
    let Some(allow_loopback) = raw.allow_loopback else {
        return Err(WindowsSandboxError::for_subcommand(
            Code::FenceHelperBadJson,
            "fence-helper status: `wfp` does not say `allow_loopback`",
            "status",
        ));
    };
    Ok(WfpStatus {
        state: raw.state,
        filters: raw.filters,
        port_range: raw.port_range.map(|(l, h)| ProxyPortRange::reported(l, h)),
        allow_loopback,
        user_sid: non_empty(raw.user_sid),
        hint: non_empty(raw.hint),
    })
}

fn map_user(raw: UserStatusWire) -> Result<SandboxUserStatus, WindowsSandboxError> {
    let seclogon = match raw.seclogon.as_deref() {
        None => None,
        Some(state) => Some(SeclogonState::parse(state).ok_or_else(|| {
            WindowsSandboxError::for_subcommand(
                Code::FenceHelperBadJson,
                format!("fence-helper status: unknown seclogon state {state:?}"),
                "status",
            )
        })?),
    };
    let bad = |why: &str| {
        WindowsSandboxError::for_subcommand(
            Code::FenceHelperBadJson,
            format!("fence-helper status: `capabilities` {why}"),
            "status",
        )
    };
    let capabilities = match raw.capabilities {
        None | Some(serde_json::Value::Null) => Vec::new(),
        Some(serde_json::Value::Array(items)) => items
            .into_iter()
            .map(|i| {
                i.as_str()
                    .map(str::to_string)
                    .ok_or_else(|| bad("has an entry that is not text"))
            })
            .collect::<Result<Vec<_>, _>>()?,
        Some(_) => return Err(bad("is not a list")),
    };
    Ok(SandboxUserStatus {
        provisioned: raw.user.exists,
        sid: non_empty(raw.user.sid),
        group_exists: raw.user.group_exists,
        group_sid: non_empty(raw.user.group_sid),
        in_builtin_users: raw.user.in_builtin_users,
        in_sandbox_group: raw.user.in_sandbox_group,
        hidden_from_logon: raw.user.hidden_from_logon,
        cred_present: raw.cred_present,
        marker_version: raw.marker_version,
        setup_version: raw.setup_version,
        real_user_sid: raw.real_user_sid,
        ca_cert_thumb: non_empty(raw.ca_cert_thumb),
        ca_cert_pem: non_empty(raw.ca_cert_pem),
        seclogon,
        // Anything but an explicit "none" counts: a state this code does not
        // know must not read as "no grants".
        persistent_grants: raw
            .persistent_grants
            .filter(|g| g.state != "none")
            .map(|g| PersistentGrants {
                state: grant_state(&g.state),
                roots: g.roots.unwrap_or_default(),
                reason: non_empty(g.reason),
            }),
        profile_grant: raw
            .profile_grant
            .filter(|g| g.state != "none")
            .map(|g| ProfileGrantRecord {
                state: grant_state(&g.state),
                reason: non_empty(g.reason),
            }),
        capabilities,
        session_user_sid: non_empty(raw.session_user_sid),
    })
}

/// Map the stdout of `fence-helper status` (`{user, wfp}`).
pub fn parse_sandbox_status(stdout: &str, args: &[String]) -> Result<SandboxStatus, WindowsSandboxError> {
    let raw: SandboxStatusWire = serde_json::from_str(stdout).map_err(|e| {
        WindowsSandboxError::for_subcommand(
            Code::FenceHelperBadJson,
            format!(
                "fence-helper {}: unparseable JSON output {}: {e}",
                args.join(" "),
                serde_json::to_string(stdout).unwrap_or_default()
            ),
            args.first().cloned().unwrap_or_default(),
        )
    })?;
    Ok(SandboxStatus {
        user: map_user(raw.user)?,
        wfp: map_wfp(raw.wfp)?,
    })
}
