use std::collections::BTreeMap;

use serde_json::error::Category::Data;
use serde_json::json;
use trogon_sandbox_contract::{
    GetSandboxDomainRequest, GetSandboxDomainResponse, LeaseError, LeaseId, NativeHandle, NativeHandleRequest,
    NativeHandleResponse, PortAccess, ProviderKind, SshAccess, SshAccessRequest, SshAccessResponse,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn error_of<T, E>(result: Result<T, E>) -> Result<E, Box<dyn std::error::Error>> {
    result.err().ok_or_else(|| "expected an error".into())
}

fn rejects<T: serde::de::DeserializeOwned>(value: serde_json::Value) -> TestResult {
    assert_eq!(error_of(serde_json::from_value::<T>(value.clone()))?.classify(), Data);
    assert_eq!(
        error_of(serde_json::from_str::<T>(&value.to_string()))?.classify(),
        Data
    );
    Ok(())
}

fn round_trip<T>(value: &T, wire: serde_json::Value) -> TestResult
where
    T: serde::Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug,
{
    assert_eq!(serde_json::to_value(value)?, wire);
    assert_eq!(&serde_json::from_value::<T>(wire.clone())?, value);
    assert_eq!(&serde_json::from_str::<T>(&wire.to_string())?, value);
    Ok(())
}

fn denial() -> LeaseError {
    LeaseError::LeaseNotFound {
        lease_id: LeaseId::new("lease-1"),
    }
}

#[test]
fn a_domain_request_omits_an_unset_wait() -> TestResult {
    assert_eq!(GetSandboxDomainRequest::MAX_WAIT_MS, 120_000);
    let bare = GetSandboxDomainRequest {
        lease_id: LeaseId::new("lease-1"),
        port: 8080,
        wait_ms: None,
    };
    round_trip(&bare, json!({"lease_id": "lease-1", "port": 8080}))?;
    let waiting = GetSandboxDomainRequest {
        wait_ms: Some(500),
        ..bare
    };
    round_trip(&waiting, json!({"lease_id": "lease-1", "port": 8080, "wait_ms": 500}))?;
    rejects::<GetSandboxDomainRequest>(json!({"lease_id": "lease-1", "port": 70000}))
}

#[test]
fn port_access_omits_empty_headers() -> TestResult {
    let bare = PortAccess {
        base_url: "https://x.test".to_string(),
        headers: BTreeMap::new(),
    };
    round_trip(&bare, json!({"base_url": "https://x.test"}))?;
    let with_headers = PortAccess {
        headers: BTreeMap::from([("x-token".to_string(), "t".to_string())]),
        ..bare
    };
    round_trip(
        &with_headers,
        json!({"base_url": "https://x.test", "headers": {"x-token": "t"}}),
    )
}

#[test]
fn every_domain_outcome_is_tagged() -> TestResult {
    round_trip(
        &GetSandboxDomainResponse::Available {
            endpoint: "e.test:1".to_string(),
            url: None,
            expires_at: None,
            access: None,
        },
        json!({"outcome": "available", "endpoint": "e.test:1"}),
    )?;
    round_trip(
        &GetSandboxDomainResponse::Available {
            endpoint: "e.test:1".to_string(),
            url: Some("https://e.test".to_string()),
            expires_at: Some(99),
            access: Some(PortAccess {
                base_url: "https://e.test".to_string(),
                headers: BTreeMap::new(),
            }),
        },
        json!({
            "outcome": "available",
            "endpoint": "e.test:1",
            "url": "https://e.test",
            "expires_at": 99,
            "access": {"base_url": "https://e.test"},
        }),
    )?;
    round_trip(
        &GetSandboxDomainResponse::PortNotExposed,
        json!({"outcome": "port_not_exposed"}),
    )?;
    round_trip(
        &GetSandboxDomainResponse::Unsupported,
        json!({"outcome": "unsupported"}),
    )?;
    round_trip(
        &GetSandboxDomainResponse::NotReady { waited_ms: 30 },
        json!({"outcome": "not_ready", "waited_ms": 30}),
    )?;
    let denied = GetSandboxDomainResponse::Denied(denial());
    let wire = serde_json::to_value(&denied)?;
    assert_eq!(wire["outcome"], json!("denied"));
    assert_eq!(serde_json::from_value::<GetSandboxDomainResponse>(wire)?, denied);
    rejects::<GetSandboxDomainResponse>(json!({"outcome": "gone"}))?;
    rejects::<GetSandboxDomainResponse>(json!({"outcome": "not_ready"}))
}

#[test]
fn a_native_handle_request_names_the_lease() -> TestResult {
    round_trip(
        &NativeHandleRequest {
            lease_id: LeaseId::new("lease-1"),
        },
        json!({"lease_id": "lease-1"}),
    )
}

#[test]
fn a_native_handle_omits_unset_parts() -> TestResult {
    let bare = NativeHandle {
        provider: ProviderKind::LocalDocker,
        vendor_id: "v-1".to_string(),
        url: None,
        region: None,
    };
    round_trip(&bare, json!({"provider": "local_docker", "vendor_id": "v-1"}))?;
    let full = NativeHandle {
        url: Some("https://v.test".to_string()),
        region: Some("eu".to_string()),
        ..bare
    };
    round_trip(
        &full,
        json!({"provider": "local_docker", "vendor_id": "v-1", "url": "https://v.test", "region": "eu"}),
    )?;
    rejects::<NativeHandle>(json!({"provider": "nope", "vendor_id": "v"}))
}

#[test]
fn every_native_handle_outcome_is_tagged() -> TestResult {
    let handle = NativeHandle {
        provider: ProviderKind::Subprocess,
        vendor_id: "v-1".to_string(),
        url: None,
        region: None,
    };
    round_trip(
        &NativeHandleResponse::Available(handle),
        json!({"outcome": "available", "provider": "subprocess", "vendor_id": "v-1"}),
    )?;
    round_trip(&NativeHandleResponse::Unsupported, json!({"outcome": "unsupported"}))?;
    let denied = NativeHandleResponse::Denied(denial());
    let wire = serde_json::to_value(&denied)?;
    assert_eq!(wire["outcome"], json!("denied"));
    assert_eq!(serde_json::from_value::<NativeHandleResponse>(wire)?, denied);
    rejects::<NativeHandleResponse>(json!({"outcome": "gone"}))
}

#[test]
fn an_ssh_request_omits_an_unset_ttl() -> TestResult {
    let bare = SshAccessRequest {
        lease_id: LeaseId::new("lease-1"),
        ttl_seconds: None,
    };
    round_trip(&bare, json!({"lease_id": "lease-1"}))?;
    let ttl = SshAccessRequest {
        ttl_seconds: Some(60),
        ..bare
    };
    round_trip(&ttl, json!({"lease_id": "lease-1", "ttl_seconds": 60}))?;
    rejects::<SshAccessRequest>(json!({"lease_id": "lease-1", "ttl_seconds": -1}))
}

#[test]
fn every_ssh_outcome_is_tagged() -> TestResult {
    let bare = SshAccess {
        host: "h.test".to_string(),
        port: 22,
        user: "root".to_string(),
        command: None,
        expires_at: None,
    };
    round_trip(&bare, json!({"host": "h.test", "port": 22, "user": "root"}))?;
    let full = SshAccess {
        command: Some("ssh root@h.test".to_string()),
        expires_at: Some(7),
        ..bare.clone()
    };
    round_trip(
        &full,
        json!({"host": "h.test", "port": 22, "user": "root", "command": "ssh root@h.test", "expires_at": 7}),
    )?;
    round_trip(
        &SshAccessResponse::Available(bare),
        json!({"outcome": "available", "host": "h.test", "port": 22, "user": "root"}),
    )?;
    round_trip(&SshAccessResponse::Unsupported, json!({"outcome": "unsupported"}))?;
    let denied = SshAccessResponse::Denied(denial());
    let wire = serde_json::to_value(&denied)?;
    assert_eq!(wire["outcome"], json!("denied"));
    assert_eq!(serde_json::from_value::<SshAccessResponse>(wire)?, denied);
    rejects::<SshAccessResponse>(json!({"outcome": "gone"}))
}
