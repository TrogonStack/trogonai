use crate::constants::WINDOWS_ALLOW_LOOPBACK_EFFECT;
use crate::install_profile_grant::InstallProfileGrant;
use crate::sandbox_status::SandboxStatus;

/// What `windows-install` prints on success.
pub fn render_install_report(status: &SandboxStatus, grant: &InstallProfileGrant) -> String {
    let mut out = format!(
        "Installed.\n  sandbox user: {}",
        if status.user.provisioned {
            "provisioned"
        } else {
            "MISSING"
        }
    );
    if let Some(sid) = &status.user.sid {
        out.push_str(&format!(" ({sid})"));
    }
    out.push_str(&format!(
        "\n  WFP:   {}, {} filters",
        status.wfp.state.as_str(),
        status.wfp.filters
    ));
    if let Some(range) = &status.wfp.port_range {
        out.push_str(&format!(", port range {range}"));
    }
    out.push_str("\n  profile read grant: ");
    match grant {
        InstallProfileGrant::Applied { result, .. } => out.push_str(&format!(
            "applied to {} ({} directories, {} excluded, {} skipped)",
            result.profile,
            result.granted,
            result.denied,
            result.skipped.len()
        )),
        InstallProfileGrant::Skipped { reason } => {
            out.push_str(&format!("NOT applied - {reason}"));
        }
    }
    out.push_str(
        "\n\nNo logout needed - the WFP filter keys on the dedicated `trogon-sandbox` user's SID, so \
         your network is unaffected.",
    );
    if status.wfp.allow_loopback {
        out.push_str(&format!("\n\n{WINDOWS_ALLOW_LOOPBACK_EFFECT}"));
    }
    out
}
