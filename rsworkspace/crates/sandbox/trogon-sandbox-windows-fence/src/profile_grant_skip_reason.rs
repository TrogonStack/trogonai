use crate::constants::SERVICE_ACCOUNT_SIDS;
use crate::sandbox_user_status::SandboxUserStatus;

/// Why an install leaves the profile read grant unapplied (`None`: it applies
/// it): asked to skip, a service account, or not running as the owner of the
/// interactive session, whose profile it would open instead.
pub fn profile_grant_skip_reason(user: &SandboxUserStatus, skip_requested: bool) -> Option<String> {
    if skip_requested {
        return Some("--skip-profile-grant was set".to_string());
    }
    let how = "run the install, or `fence-helper profile grant`, as the user whose sessions are to \
               read their profile";
    let real = &user.real_user_sid;
    if SERVICE_ACCOUNT_SIDS.contains(&real.as_str()) {
        return Some(format!(
            "the install ran as the service account {real}, which runs no sessions; {how}"
        ));
    }
    let Some(session) = &user.session_user_sid else {
        return Some(format!(
            "the owner of the session this install ran in could not be determined, so it is not \
             known whose profile {real} is; {how}"
        ));
    };
    if !session.eq_ignore_ascii_case(real) {
        return Some(format!(
            "the install ran as {real} in a session that belongs to {session}; granting would \
             open the installing account's profile, not the session owner's; {how}"
        ));
    }
    None
}
