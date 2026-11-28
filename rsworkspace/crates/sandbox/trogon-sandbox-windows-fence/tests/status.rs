use trogon_sandbox_windows_fence::{
    GrantState, LeaseProxyPortsError, SandboxStatus, SeclogonState, WfpState, WindowsSandboxErrorCode as Code,
    parse_sandbox_status,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const FULL: &str = r#"{
  "user": {
    "user": {"exists": true, "sid": "S-1-5-21-1-2-3-1001", "group_exists": true,
             "group_sid": "S-1-5-21-1-2-3-1002", "in_builtin_users": false,
             "in_sandbox_group": true, "hidden_from_logon": true},
    "cred_present": false, "marker_version": 4, "setup_version": 4,
    "real_user_sid": "S-1-5-21-1-2-3-1000", "ca_cert_thumb": "", "ca_cert_pem": null,
    "seclogon": "ok",
    "persistent_grants": {"state": "present", "roots": ["C:\\w"]},
    "profile_grant": {"state": "weird"},
    "session_user_sid": "S-1-5-21-1-2-3-1000",
    "capabilities": ["exec-holder", "kept-grant-holders"]
  },
  "wfp": {"state": "installed", "filters": 6, "port_range": [60080, 60089],
          "allow_loopback": true, "user_sid": "S-1-5-21-1-2-3-1001"}
}"#;

fn args() -> Vec<String> {
    vec!["status".to_string()]
}

#[test]
fn maps_every_field() -> TestResult {
    let s = parse_sandbox_status(FULL, &args())?;
    assert!(s.user.provisioned);
    assert_eq!(s.user.sid.as_deref(), Some("S-1-5-21-1-2-3-1001"));
    assert_eq!(s.user.ca_cert_thumb, None);
    assert_eq!(s.user.ca_cert_pem, None);
    assert_eq!(s.user.ca_cert(), None);
    let mut user = s.user.clone();
    user.ca_cert_pem = Some("PEM".to_string());
    assert_eq!(user.ca_cert(), None);
    user.ca_cert_thumb = Some("AB".to_string());
    assert_eq!(user.ca_cert(), Some(("PEM", "AB")));
    assert_eq!(s.user.seclogon, Some(SeclogonState::Ok));
    let g = s.user.persistent_grants.ok_or("no persistent grants")?;
    assert_eq!((g.state, g.roots), (GrantState::Present, vec!["C:\\w".to_string()]));
    assert_eq!(
        s.user.profile_grant.ok_or("no profile grant")?.state,
        GrantState::Unknown
    );
    assert_eq!(s.user.capabilities, ["exec-holder", "kept-grant-holders"]);
    assert_eq!(s.wfp.state, WfpState::Installed);
    assert_eq!(s.wfp.port_range.ok_or("no port range")?.to_string(), "60080-60089");
    assert!(s.wfp.allow_loopback);
    Ok(())
}

#[test]
fn none_states_and_missing_fields() -> TestResult {
    let json = r#"{"user":{"user":{"exists":false,"group_exists":false,"in_builtin_users":false,
      "in_sandbox_group":false,"hidden_from_logon":false},"cred_present":false,
      "real_user_sid":"S-1","persistent_grants":{"state":"none"},
      "profile_grant":{"state":"none"}},
      "wfp":{"state":"cannot-read","filters":0,"allow_loopback":false,"hint":"run elevated"}}"#;
    let s = parse_sandbox_status(json, &args())?;
    assert!(!s.user.provisioned);
    assert_eq!(s.user.seclogon, None);
    assert_eq!(s.user.persistent_grants, None);
    assert_eq!(s.user.profile_grant, None);
    assert!(s.user.capabilities.is_empty());
    assert_eq!(s.wfp.state, WfpState::CannotRead);
    assert!(!s.wfp.allow_loopback);
    assert_eq!(s.wfp.hint.as_deref(), Some("run elevated"));
    Ok(())
}

#[test]
fn bad_json_is_typed() -> TestResult {
    let Err(e) = parse_sandbox_status("nope", &args()) else {
        return Err("expected an error".into());
    };
    assert_eq!(e.code(), Code::FenceHelperBadJson);
    assert!(
        e.message()
            .starts_with("fence-helper status: unparseable JSON output \"nope\"")
    );
    Ok(())
}

#[test]
fn bad_json_without_args_has_no_subcommand() -> TestResult {
    let Err(e) = parse_sandbox_status("nope", &[]) else {
        return Err("expected an error".into());
    };
    assert_eq!(e.code(), Code::FenceHelperBadJson);
    Ok(())
}

fn with(edit: impl FnOnce(&mut SandboxStatus)) -> Result<SandboxStatus, Box<dyn std::error::Error>> {
    let mut status = parse_sandbox_status(FULL, &args())?;
    status.wfp.allow_loopback = false;
    edit(&mut status);
    Ok(status)
}

#[test]
fn leases_use_the_ports_of_a_closed_installed_fence() -> TestResult {
    let ports = with(|_| {})?.lease_proxy_ports()?;
    assert_eq!(ports.to_string(), "60080-60089");
    Ok(())
}

#[test]
fn leases_are_refused_when_the_account_is_not_provisioned() -> TestResult {
    let status = with(|s| s.user.provisioned = false)?;
    assert_eq!(status.lease_proxy_ports(), Err(LeaseProxyPortsError::NotProvisioned));
    Ok(())
}

#[test]
fn leases_are_refused_when_the_filters_are_not_installed() -> TestResult {
    let status = with(|s| s.wfp.state = WfpState::Absent)?;
    let err = status.lease_proxy_ports();
    assert_eq!(
        err,
        Err(LeaseProxyPortsError::FiltersNotInstalled {
            state: WfpState::Absent.as_str().to_string()
        })
    );
    Ok(())
}

#[test]
fn leases_are_refused_when_loopback_is_open() -> TestResult {
    let status = with(|s| s.wfp.allow_loopback = true)?;
    let Err(e) = status.lease_proxy_ports() else {
        return Err("expected an error".into());
    };
    assert_eq!(e, LeaseProxyPortsError::LoopbackOpen);
    Ok(())
}

#[test]
fn leases_are_refused_when_no_port_range_is_reported() -> TestResult {
    let status = with(|s| s.wfp.port_range = None)?;
    assert_eq!(status.lease_proxy_ports(), Err(LeaseProxyPortsError::NoPortRange));
    Ok(())
}

#[test]
fn a_status_without_allow_loopback_is_refused() -> TestResult {
    let json = FULL.replace("\"allow_loopback\": true, ", "");
    let Err(e) = parse_sandbox_status(&json, &args()) else {
        return Err("expected an error".into());
    };
    assert_eq!(e.code(), Code::FenceHelperBadJson);
    assert!(e.message().contains("does not say `allow_loopback`"));
    Ok(())
}
