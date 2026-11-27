mod support;

use support::*;
use trogon_sandbox_modal_worker::{
    CommitSha, CommitShaError, CpuCores, CpuCoresError, ModalApiError, ModalAppName, ModalAppNameError, ModalSandbox,
    ModalSandboxSpec, ModalTunnel, WorkspaceError,
};

#[test]
fn core_counts_are_positive_and_finite() -> TestResult {
    assert!((CpuCores::new(0.5)?.get() - 0.5).abs() < f64::EPSILON);
    assert!((CpuCores::from_millis(250)?.get() - 0.25).abs() < f64::EPSILON);
    assert!((CpuCores::from_millis(1500)?.get() - 1.5).abs() < f64::EPSILON);
    assert!(matches!(CpuCores::new(0.0), Err(CpuCoresError::NotPositive)));
    assert!(matches!(CpuCores::new(-1.0), Err(CpuCoresError::NotPositive)));
    assert!(matches!(CpuCores::new(f64::NAN), Err(CpuCoresError::NotPositive)));
    assert!(matches!(CpuCores::new(f64::INFINITY), Err(CpuCoresError::NotPositive)));
    assert!(matches!(CpuCores::from_millis(0), Err(CpuCoresError::NotPositive)));
    assert!((CpuCores::from_str_value("2.5")?.get() - 2.5).abs() < f64::EPSILON);
    assert!(matches!("two".parse::<CpuCores>(), Err(CpuCoresError::Unparsable(_))));
    assert!(matches!("0".parse::<CpuCores>(), Err(CpuCoresError::NotPositive)));
    assert_eq!(serde_json::to_string(&CpuCores::new(2.0)?)?, "2.0");
    Ok(())
}

trait ParseValue: std::str::FromStr {
    fn from_str_value(value: &str) -> Result<Self, Self::Err> {
        value.parse()
    }
}

impl ParseValue for CpuCores {}

#[test]
fn app_names_are_non_empty_and_have_no_whitespace() -> TestResult {
    let name = ModalAppName::new("trogon-sandboxes")?;
    assert_eq!(name.as_str(), "trogon-sandboxes");
    assert_eq!(format!("{name}"), "trogon-sandboxes");
    assert_eq!("other".parse::<ModalAppName>()?.as_str(), "other");
    assert!(matches!(ModalAppName::new(""), Err(ModalAppNameError::Empty)));
    assert!(matches!(ModalAppName::new("two words"), Err(ModalAppNameError::Unsafe)));
    assert!(matches!(ModalAppName::new("tab\there"), Err(ModalAppNameError::Unsafe)));
    assert_eq!(serde_json::to_string(&name)?, "\"trogon-sandboxes\"");
    Ok(())
}

#[test]
fn commit_ids_are_short_or_long_hex() -> TestResult {
    let sha = CommitSha::new("abc123")?;
    assert_eq!(sha.as_str(), "abc123");
    assert_eq!(format!("{sha}"), "abc123");
    assert_eq!("DEADBEEF".parse::<CommitSha>()?.as_str(), "DEADBEEF");
    assert!(matches!(CommitSha::new("abc"), Err(CommitShaError::Length { .. })));
    assert!(matches!(
        CommitSha::new("a".repeat(65)),
        Err(CommitShaError::Length { .. })
    ));
    assert!(matches!(CommitSha::new("--upload-pack"), Err(CommitShaError::NotHex)));
    Ok(())
}

#[test]
fn sandboxes_run_in_the_states_modal_reports_as_started() -> TestResult {
    let state = |state: &str| -> Boxed<ModalSandbox> {
        Ok(serde_json::from_value(
            serde_json::json!({"id": "sb-1", "state": state}),
        )?)
    };
    for running in ["running", "Ready", "STARTED", "active"] {
        assert!(state(running)?.is_running());
    }
    for idle in ["created", "finished", "scheduled"] {
        assert!(!state(idle)?.is_running());
    }
    let finished: ModalSandbox =
        serde_json::from_value(serde_json::json!({"id": "sb-1", "state": "finished", "exit_code": 2}))?;
    assert_eq!(finished.exit_code, Some(2));
    Ok(())
}

#[test]
fn bridge_values_deserialize_and_specs_serialize_without_unset_fields() -> TestResult {
    let tunnel: ModalTunnel =
        serde_json::from_value(serde_json::json!({"port": 8080, "host": "h", "tls_port": 443, "url": "https://h"}))?;
    assert_eq!(tunnel.tls_port, 443);
    let spec = ModalSandboxSpec {
        app_name: ModalAppName::new("app")?,
        image: "ubuntu".to_owned(),
        image_id: None,
        environment: None,
        workdir: "/w".to_owned(),
        timeout_seconds: 5,
        cpu_cores: Some(CpuCores::new(1.0)?),
        memory_mib: None,
        region: None,
        env: Vec::new(),
        tags: Vec::new(),
        block_network: false,
        outbound_cidr_allowlist: Vec::new(),
        encrypted_ports: Vec::new(),
    };
    let wire = serde_json::to_value(&spec)?;
    for absent in ["image_id", "environment", "memory_mib", "region", "encrypted_ports"] {
        assert!(wire.get(absent).is_none(), "{absent} should be omitted");
    }
    assert_eq!(wire["cpu_cores"], 1.0);
    Ok(())
}

#[test]
fn errors_describe_themselves() -> TestResult {
    let io = || std::io::Error::other("disk");
    let described: Vec<String> = vec![
        ModalApiError::Modal {
            kind: "K".into(),
            message: "m".into(),
        }
        .to_string(),
        ModalApiError::SandboxNotFound("sb".into()).to_string(),
        ModalApiError::SandboxNotReady { attempts: 3 }.to_string(),
        ModalApiError::Auth("token".into()).to_string(),
        ModalApiError::NoOutput.to_string(),
        ModalApiError::StdinUnavailable.to_string(),
        ModalApiError::Spawn {
            python: "py".into(),
            source: io(),
        }
        .to_string(),
        ModalApiError::Exited {
            op: "get".into(),
            status: "1".into(),
            stderr: "e".into(),
        }
        .to_string(),
        ModalApiError::from(io()).to_string(),
        WorkspaceError::HostMountUnsupported.to_string(),
        WorkspaceError::SecretUnresolved.to_string(),
        WorkspaceError::CloneScheme("ext".into()).to_string(),
        WorkspaceError::CloneCredentials.to_string(),
        WorkspaceError::ScopeUnsupported.to_string(),
        WorkspaceError::CredentialToken.to_string(),
        WorkspaceError::from(io()).to_string(),
        CommitShaError::NotHex.to_string(),
        CpuCoresError::NotPositive.to_string(),
        ModalAppNameError::Empty.to_string(),
        ModalAppNameError::Unsafe.to_string(),
    ];
    assert!(described.iter().all(|text| !text.is_empty()));
    Ok(())
}
