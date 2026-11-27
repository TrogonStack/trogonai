//! End-to-end: credential injection (`Transform`) on a networked Firecracker lease, through the broker and a live
//! credential broker. The proxy fetches the credential per request and sets the header, so the upstream sees the value
//! and the guest never does.
//!
//! Prereqs: everything `firecracker_stack_smoke` needs, a `trogon-sandbox-credential-broker` on the same NATS, a worker
//! started with `SANDBOX_FIRECRACKER_CREDENTIAL_BROKER_NATS_URL` naming it and with lease networking, a registry token
//! credential stored for `smoke-tenant` as `smoke-injected` whose
//! token is `SANDBOX_SMOKE_FIRECRACKER_CREDENTIAL_SECRET`, and a host that can reach
//! `SANDBOX_SMOKE_FIRECRACKER_ECHO_HOST` (default `postman-echo.com`), a service that answers `GET /headers` with the
//! request headers as JSON.

mod common;
mod firecracker_support;
mod stack_support;

use common::TestResult;
use firecracker_support::{Fallible, lease_request, sh_within, spec_parts, with_lease};
use trogon_sandbox_contract::{
    CredentialInjection, CredentialProvider, CredentialRequest, CredentialScope, CredentialTransform, DomainPattern,
    DomainRule, DomainRuleAction, HeaderName, LeaseRequest, NetworkPolicy, OpaqueTunnelPolicy, PullSecretId,
};

fn transform_policy(host: &str) -> Fallible<NetworkPolicy> {
    Ok(NetworkPolicy::UserDefined {
        rules: vec![DomainRule {
            domain: DomainPattern::new(host)?,
            action: DomainRuleAction::Transform(CredentialTransform {
                match_rules: vec![],
                credential: CredentialRequest {
                    provider: CredentialProvider::Registry,
                    pull_secret_id: PullSecretId::new("smoke-injected"),
                    scope: CredentialScope::RegistryPull {
                        repository_glob: "*".to_string(),
                    },
                    ttl_seconds: 60,
                    requester: None,
                },
                injection: CredentialInjection::Header(HeaderName::new("X-Injected")?),
            }),
        }],
        allow_ranges: vec![],
        deny_ranges: vec![],
        denied_domains: vec![],
        opaque_tunnels: OpaqueTunnelPolicy::default(),
        request_rules: vec![],
    })
}

fn request(network_policy: NetworkPolicy) -> Fallible<LeaseRequest> {
    let mut parts = spec_parts(30, 256)?;
    parts.network_policy = Some(network_policy);
    lease_request("smoke-tenant", parts, 180, "cred-smoke")
}

#[tokio::test]
#[ignore = "requires a networked firecracker worker with a credential broker; run devops/docker/compose/sandbox/smoke-firecracker.sh"]
async fn a_transform_rule_injects_the_credential_upstream_and_the_guest_never_sees_it() -> TestResult {
    let client = &stack_support::connect().await?;
    let host = stack_support::env_or("SANDBOX_SMOKE_FIRECRACKER_ECHO_HOST", "postman-echo.com");
    let secret = stack_support::required_env("SANDBOX_SMOKE_FIRECRACKER_CREDENTIAL_SECRET")?;
    let req = request(transform_policy(&host)?)?;

    with_lease(client, req, |granted| async move {
        let id = &granted.lease_id;
        let echoed = sh_within(client, id, &format!("curl -sS -m 30 https://{host}/headers"), 60).await?;
        let echoed_body = stack_support::stdout(&echoed);
        // The guest holds neither the value nor the header it was not told to send.
        let guest_view = sh_within(
            client,
            id,
            &format!(
                "env; cat /proc/self/environ | tr '\\0' '\\n'; \
                 grep -rIl --exclude-dir=proc --exclude-dir=sys --exclude-dir=dev -F '{secret}' /etc /root /home /run /tmp /workspace 2>/dev/null; true"
            ),
            60,
        )
        .await?;
        let guest_view_body = stack_support::stdout(&guest_view);

        assert_eq!(echoed.exit_code, 0, "{echoed:?}");
        assert!(
            echoed_body.to_lowercase().contains("x-injected") && echoed_body.contains(&secret),
            "the upstream did not see the injected header: {echoed_body}"
        );
        assert!(
            !guest_view_body.contains(&secret),
            "the credential reached the guest: {guest_view_body}"
        );
        Ok(())
    })
    .await
}
