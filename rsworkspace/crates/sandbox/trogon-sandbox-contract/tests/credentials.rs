use std::num::NonZeroU32;
use std::str::FromStr;
use std::time::{Duration, SystemTime};

use serde_json::error::Category::Data;
use serde_json::json;
use trogon_sandbox_contract::{
    CredentialAck, CredentialAckError, CredentialAckOutcome, CredentialAckStatus, CredentialAuditEvent,
    CredentialError, CredentialId, CredentialIssueOutcome, CredentialIssueRequest, CredentialIssueResponse,
    CredentialProvider, CredentialProviderError, CredentialRevokeRequest, CredentialRotateRequest, CredentialScope,
    LeaseId, OidcClaims, OidcToken, PolicyFence, PullSecretId, SandboxId, ServiceAccountId, ShortLivedToken, TenantId,
    TokenId,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const SECRET: &str = "hunter2-token";

fn time(secs: u64) -> serde_json::Value {
    json!({ "secs_since_epoch": secs, "nanos_since_epoch": 0 })
}

fn at(secs: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
}

#[test]
fn credential_and_token_ids_are_unchecked_strings() -> TestResult {
    for value in ["cred-1", ""] {
        let credential = CredentialId::new(value);
        assert_eq!(credential.as_str(), value);
        assert_eq!(credential.to_string(), value);
        assert_eq!(CredentialId::from(value.to_owned()), credential);
        assert_eq!(serde_json::to_value(&credential)?, json!(value));
        assert_eq!(serde_json::from_value::<CredentialId>(json!(value))?, credential);
        assert_eq!(String::from(credential), value);

        let token = TokenId::new(value);
        assert_eq!(token.as_str(), value);
        assert_eq!(token.to_string(), value);
        assert_eq!(TokenId::from(value.to_owned()), token);
        assert_eq!(serde_json::to_value(&token)?, json!(value));
        assert_eq!(serde_json::from_value::<TokenId>(json!(value))?, token);
        assert_eq!(String::from(token), value);
    }
    Ok(())
}

#[test]
fn tokens_never_show_their_value_in_debug() -> TestResult {
    let token = ShortLivedToken::new(SECRET);
    assert_eq!(token.expose_secret(), SECRET);
    assert_eq!(format!("{token:?}"), "ShortLivedToken([REDACTED])");
    assert_eq!(ShortLivedToken::from(SECRET.to_owned()), token);
    assert_eq!(serde_json::to_value(&token)?, json!(SECRET));
    assert_eq!(serde_json::from_value::<ShortLivedToken>(json!(SECRET))?, token);
    assert_eq!(String::from(token), SECRET);

    let oidc = OidcToken::new(SECRET);
    assert_eq!(oidc.expose_secret(), SECRET);
    assert_eq!(format!("{oidc:?}"), "OidcToken([REDACTED])");
    assert_eq!(OidcToken::from(SECRET.to_owned()), oidc);
    assert_eq!(serde_json::to_value(&oidc)?, json!(SECRET));
    assert_eq!(serde_json::from_value::<OidcToken>(json!(SECRET))?, oidc);
    assert_eq!(String::from(oidc), SECRET);
    Ok(())
}

#[test]
fn oidc_claims_name_the_forward_target_and_the_lease() -> TestResult {
    let claims = OidcClaims {
        iss: "https://egress.example".to_owned(),
        sub: SandboxId::new("sandbox-1")?,
        aud: "https://forward.example/hooks".to_owned(),
        tenant_id: TenantId::new("tenant-a")?,
        lease_id: LeaseId::new("lease-1"),
        iat: 100,
        exp: 400,
    };
    let encoded = json!({
        "iss": "https://egress.example",
        "sub": "sandbox-1",
        "aud": "https://forward.example/hooks",
        "tenant_id": "tenant-a",
        "lease_id": "lease-1",
        "iat": 100,
        "exp": 400,
    });
    assert_eq!(serde_json::to_value(&claims)?, encoded);
    assert_eq!(serde_json::from_value::<OidcClaims>(encoded)?, claims);
    Ok(())
}

#[test]
fn credential_providers_parse_their_wire_names() -> TestResult {
    for (provider, name) in [
        (CredentialProvider::Registry, "registry"),
        (CredentialProvider::S3, "s3"),
        (CredentialProvider::Daytona, "daytona"),
        (CredentialProvider::Modal, "modal"),
        (CredentialProvider::Azure, "azure"),
        (CredentialProvider::Docker, "docker"),
        (CredentialProvider::Ssh, "ssh"),
    ] {
        assert_eq!(provider.as_str(), name);
        assert_eq!(provider.to_string(), name);
        assert_eq!(CredentialProvider::from_str(name)?, provider);
        assert_eq!(serde_json::to_value(provider)?, json!(name));
        assert_eq!(serde_json::from_value::<CredentialProvider>(json!(name))?, provider);
    }
    assert_eq!(
        CredentialProvider::from_str("S3").err(),
        Some(CredentialProviderError::Unknown)
    );
    assert_eq!(
        CredentialProviderError::Unknown.to_string(),
        "unknown credential provider"
    );
    Ok(())
}

#[test]
fn credential_scopes_are_tagged_by_kind() -> TestResult {
    let cases = [
        (
            CredentialScope::RegistryPull {
                repository_glob: "team/*".to_owned(),
            },
            json!({ "kind": "registry_pull", "repository_glob": "team/*" }),
        ),
        (
            CredentialScope::RegistryPush {
                repository_glob: "team/app".to_owned(),
            },
            json!({ "kind": "registry_push", "repository_glob": "team/app" }),
        ),
        (
            CredentialScope::S3ReadOnly {
                bucket: "snapshots".to_owned(),
                prefix: "tenant-a/".to_owned(),
            },
            json!({ "kind": "s3_read_only", "bucket": "snapshots", "prefix": "tenant-a/" }),
        ),
        (
            CredentialScope::S3ReadWrite {
                bucket: "snapshots".to_owned(),
                prefix: "tenant-a/".to_owned(),
            },
            json!({ "kind": "s3_read_write", "bucket": "snapshots", "prefix": "tenant-a/" }),
        ),
        (
            CredentialScope::DaytonaApi {
                workspace_id: "ws-1".to_owned(),
            },
            json!({ "kind": "daytona_api", "workspace_id": "ws-1" }),
        ),
        (
            CredentialScope::AzureServicePrincipal {
                resource_id: "rg/app".to_owned(),
            },
            json!({ "kind": "azure_service_principal", "resource_id": "rg/app" }),
        ),
        (
            CredentialScope::SshSession {
                lease_id: LeaseId::new("lease-1"),
            },
            json!({ "kind": "ssh_session", "lease_id": "lease-1" }),
        ),
    ];
    for (scope, encoded) in cases {
        assert_eq!(serde_json::to_value(&scope)?, encoded);
        assert_eq!(serde_json::from_value::<CredentialScope>(encoded)?, scope);
    }
    Ok(())
}

#[test]
fn issue_requests_carry_an_optional_fence_and_a_nonzero_ttl() -> TestResult {
    let ttl = NonZeroU32::new(300).ok_or("ttl is nonzero")?;
    let mut request = CredentialIssueRequest {
        pull_secret_id: PullSecretId::new("registry-main"),
        scope: CredentialScope::RegistryPull {
            repository_glob: "team/*".to_owned(),
        },
        ttl_seconds: ttl,
        requester: Some(ServiceAccountId::new("builder")),
        fence: Some(PolicyFence {
            lease_id: LeaseId::new("lease-1"),
            policy_revision: 4,
        }),
    };
    let encoded = json!({
        "pull_secret_id": "registry-main",
        "scope": { "kind": "registry_pull", "repository_glob": "team/*" },
        "ttl_seconds": 300,
        "requester": "builder",
        "fence": { "lease_id": "lease-1", "policy_revision": 4 },
    });
    assert_eq!(serde_json::to_value(&request)?, encoded);
    assert_eq!(serde_json::from_value::<CredentialIssueRequest>(encoded)?, request);

    request.requester = None;
    request.fence = None;
    let bare = json!({
        "pull_secret_id": "registry-main",
        "scope": { "kind": "registry_pull", "repository_glob": "team/*" },
        "ttl_seconds": 300,
    });
    assert_eq!(serde_json::to_value(&request)?, bare);
    assert_eq!(serde_json::from_value::<CredentialIssueRequest>(bare.clone())?, request);

    let mut zero = bare;
    zero["ttl_seconds"] = json!(0);
    let error = serde_json::from_value::<CredentialIssueRequest>(zero)
        .err()
        .ok_or("zero ttl deserialized")?;
    assert_eq!(error.classify(), Data);
    Ok(())
}

#[test]
fn issue_outcomes_carry_the_token_or_the_error() -> TestResult {
    let response = CredentialIssueResponse {
        token_id: TokenId::new("token-1"),
        token: ShortLivedToken::new(SECRET),
        expires_at: at(900),
    };
    let ok = CredentialIssueOutcome::Ok(response);
    let encoded = json!({ "outcome": "ok", "token_id": "token-1", "token": SECRET, "expires_at": time(900) });
    assert_eq!(serde_json::to_value(&ok)?, encoded);
    assert_eq!(serde_json::from_value::<CredentialIssueOutcome>(encoded)?, ok);

    let err = CredentialIssueOutcome::Err(CredentialError::ScopeDenied {
        message: "push is not allowed".to_owned(),
    });
    let encoded = json!({ "outcome": "err", "kind": "scope_denied", "message": "push is not allowed" });
    assert_eq!(serde_json::to_value(&err)?, encoded);
    assert_eq!(serde_json::from_value::<CredentialIssueOutcome>(encoded)?, err);
    Ok(())
}

#[test]
fn revoke_and_rotate_requests_round_trip() -> TestResult {
    let revoke = CredentialRevokeRequest {
        token_id: TokenId::new("token-1"),
        reason: "lease ended".to_owned(),
    };
    let encoded = json!({ "token_id": "token-1", "reason": "lease ended" });
    assert_eq!(serde_json::to_value(&revoke)?, encoded);
    assert_eq!(serde_json::from_value::<CredentialRevokeRequest>(encoded)?, revoke);
    let unexplained = serde_json::from_value::<CredentialRevokeRequest>(json!({ "token_id": "token-1" }))?;
    assert_eq!(unexplained.reason, "");

    let rotate = CredentialRotateRequest {
        credential_id: CredentialId::new("cred-1"),
    };
    let encoded = json!({ "credential_id": "cred-1" });
    assert_eq!(serde_json::to_value(&rotate)?, encoded);
    assert_eq!(serde_json::from_value::<CredentialRotateRequest>(encoded)?, rotate);
    Ok(())
}

#[test]
fn acks_always_carry_ok_true() -> TestResult {
    let ack = CredentialAck {
        message: Some("rotated".to_owned()),
    };
    let encoded = json!({ "ok": true, "message": "rotated" });
    assert_eq!(serde_json::to_value(&ack)?, encoded);
    assert_eq!(serde_json::from_value::<CredentialAck>(encoded)?, ack);
    let silent = CredentialAck { message: None };
    assert_eq!(serde_json::to_value(&silent)?, json!({ "ok": true }));
    assert_eq!(serde_json::from_value::<CredentialAck>(json!({ "ok": true }))?, silent);

    let error = serde_json::from_value::<CredentialAck>(json!({ "ok": false }))
        .err()
        .ok_or("failed ack deserialized")?;
    assert_eq!(error.classify(), Data);
    assert_eq!(
        CredentialAckError::NotOk.to_string(),
        "a credential ack must carry `ok: true`"
    );

    let outcome = CredentialAckOutcome::Ok(silent);
    let encoded = json!({ "outcome": "ok", "ok": true });
    assert_eq!(serde_json::to_value(&outcome)?, encoded);
    assert_eq!(serde_json::from_value::<CredentialAckOutcome>(encoded)?, outcome);
    let failed = json!({ "outcome": "ok", "ok": false });
    assert!(serde_json::from_value::<CredentialAckOutcome>(failed).is_err());

    for (status, ok) in [
        (CredentialAckStatus::Accepted, true),
        (CredentialAckStatus::Refused, false),
    ] {
        assert_eq!(serde_json::to_value(status)?, json!(ok));
        assert_eq!(serde_json::from_value::<CredentialAckStatus>(json!(ok))?, status);
    }
    Ok(())
}

#[test]
fn credential_errors_are_tagged_by_kind() -> TestResult {
    let cases = [
        (
            CredentialError::NotFound {
                message: "no such credential".to_owned(),
            },
            json!({ "kind": "not_found", "message": "no such credential" }),
            "credential not found: no such credential",
        ),
        (
            CredentialError::TokenExpired {
                token_id: TokenId::new("token-1"),
            },
            json!({ "kind": "token_expired", "token_id": "token-1" }),
            "token token-1 expired",
        ),
        (
            CredentialError::TokenRevoked {
                token_id: TokenId::new("token-1"),
            },
            json!({ "kind": "token_revoked", "token_id": "token-1" }),
            "token token-1 was revoked",
        ),
        (
            CredentialError::ScopeDenied {
                message: "bucket not allowed".to_owned(),
            },
            json!({ "kind": "scope_denied", "message": "bucket not allowed" }),
            "credential scope denied: bucket not allowed",
        ),
        (
            CredentialError::PolicyFenceRefused {
                message: "stale revision".to_owned(),
            },
            json!({ "kind": "policy_fence_refused", "message": "stale revision" }),
            "policy fence refused: stale revision",
        ),
        (
            CredentialError::Backend {
                message: "store unavailable".to_owned(),
            },
            json!({ "kind": "backend", "message": "store unavailable" }),
            "credential backend failed: store unavailable",
        ),
    ];
    for (error, encoded, display) in cases {
        assert_eq!(error.to_string(), display);
        let outcome = CredentialAckOutcome::Err(error.clone());
        assert_eq!(serde_json::to_value(&error)?, encoded);
        assert_eq!(serde_json::from_value::<CredentialError>(encoded)?, error);
        let tagged = serde_json::to_value(&outcome)?;
        assert_eq!(tagged["outcome"], json!("err"));
        assert_eq!(serde_json::from_value::<CredentialAckOutcome>(tagged)?, outcome);
    }
    Ok(())
}

#[test]
fn audit_events_are_tagged_by_event() -> TestResult {
    let cases = [
        (
            CredentialAuditEvent::Issued {
                tenant: TenantId::new("tenant-a")?,
                credential_id: CredentialId::new("cred-1"),
                token_id: TokenId::new("token-1"),
                scope: CredentialScope::SshSession {
                    lease_id: LeaseId::new("lease-1"),
                },
                ttl_seconds: NonZeroU32::new(60).ok_or("ttl is nonzero")?,
                requester: ServiceAccountId::new("gateway"),
                at: at(10),
            },
            json!({
                "event": "issued",
                "tenant": "tenant-a",
                "credential_id": "cred-1",
                "token_id": "token-1",
                "scope": { "kind": "ssh_session", "lease_id": "lease-1" },
                "ttl_seconds": 60,
                "requester": "gateway",
                "at": time(10),
            }),
        ),
        (
            CredentialAuditEvent::Revoked {
                token_id: TokenId::new("token-1"),
                reason: "lease ended".to_owned(),
                at: at(20),
            },
            json!({ "event": "revoked", "token_id": "token-1", "reason": "lease ended", "at": time(20) }),
        ),
        (
            CredentialAuditEvent::Rotated {
                credential_id: CredentialId::new("cred-1"),
                new_version: 2,
                at: at(30),
            },
            json!({ "event": "rotated", "credential_id": "cred-1", "new_version": 2, "at": time(30) }),
        ),
    ];
    for (event, encoded) in cases {
        assert_eq!(serde_json::to_value(&event)?, encoded);
        assert_eq!(serde_json::from_value::<CredentialAuditEvent>(encoded)?, event);
    }
    Ok(())
}
