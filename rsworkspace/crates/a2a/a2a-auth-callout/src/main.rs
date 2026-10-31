#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

use {
    a2a_auth_callout::admission_keys::{TenantAccount, admission_keys_from_env},
    a2a_auth_callout::constants::NATS_CONNECT_TIMEOUT,
    a2a_auth_callout::credentials::mtls::MTlsVerifier,
    a2a_auth_callout::credentials::mtls::{TrustAnchorPem, X509MtlsVerifier},
    a2a_auth_callout::credentials::oidc::{JwksOidcVerifier, OidcIssuerUrl, OidcVerifier},
    a2a_auth_callout::dispatcher::{CalloutDispatcher, CalloutDispatcherConfig},
    a2a_auth_callout::{AuthCalloutWireCodec, Subscriber},
    std::sync::Arc,
    std::time::Duration,
    tracing::{info, warn},
    trogon_nats::NatsConfig,
    trogon_std::env::{ReadEnv, SystemEnv},
};

#[cfg(test)]
mod main_tests;

fn split_env_list(env: &impl ReadEnv, name: &str) -> Vec<String> {
    env.var(name)
        .ok()
        .map(|v| {
            v.split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(String::from)
                .collect()
        })
        .unwrap_or_default()
}

fn nats_config(env: &impl ReadEnv) -> Option<NatsConfig> {
    let config = NatsConfig::from_env(env);
    (!config.servers.is_empty()).then_some(config)
}

async fn build_oidc_verifier(env: &impl ReadEnv) -> Option<Arc<dyn OidcVerifier>> {
    let issuer_raw = env.var("AUTH_CALLOUT_OIDC_ISSUER").ok()?;
    let issuer = match OidcIssuerUrl::parse(&issuer_raw) {
        Ok(i) => i,
        Err(e) => {
            warn!(error = %e, "AUTH_CALLOUT_OIDC_ISSUER set but invalid; OIDC disabled");
            return None;
        }
    };
    let audiences = split_env_list(env, "AUTH_CALLOUT_OIDC_AUDIENCES");
    if audiences.is_empty() {
        warn!("AUTH_CALLOUT_OIDC_ISSUER set but AUTH_CALLOUT_OIDC_AUDIENCES empty; OIDC disabled");
        return None;
    }
    match JwksOidcVerifier::discover(issuer, audiences).await {
        Ok(v) => Some(Arc::new(v)),
        Err(e) => {
            warn!(error = %e, "OIDC discovery failed; OIDC disabled");
            None
        }
    }
}

fn build_mtls_verifier(env: &impl ReadEnv) -> Option<Arc<dyn MTlsVerifier>> {
    let path = env.var("AUTH_CALLOUT_MTLS_TRUST_ANCHORS").ok()?;
    let bundle = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) => {
            warn!(path = %path, error = %e, "failed to read mTLS trust anchors; mTLS disabled");
            return None;
        }
    };
    Some(Arc::new(X509MtlsVerifier::new(TrustAnchorPem::new(bundle))))
}

#[tokio::main]
#[cfg_attr(coverage_nightly, coverage(off))]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let env = SystemEnv;

    let Some(nats) = nats_config(&env) else {
        tracing::error!("NATS_URL must list at least one server");
        std::process::exit(1);
    };
    let admission_keys = match admission_keys_from_env(&env).await {
        Ok(keys) => keys,
        Err(e) => {
            tracing::error!(error = %e, "invalid admission key configuration");
            std::process::exit(1);
        }
    };
    let user_jwt_ttl = match env.var("AUTH_CALLOUT_USER_JWT_TTL_SECS") {
        Ok(raw) => match raw.parse::<u64>() {
            Ok(secs) => Duration::from_secs(secs),
            Err(e) => {
                tracing::error!(error = %e, raw = %raw, "invalid AUTH_CALLOUT_USER_JWT_TTL_SECS");
                std::process::exit(1);
            }
        },
        Err(_) => Duration::from_secs(a2a_auth_callout::constants::DEFAULT_USER_JWT_TTL_SECS),
    };

    let wire = AuthCalloutWireCodec::new(
        admission_keys.auth_account().clone(),
        admission_keys.response_signing_key().clone(),
        admission_keys.callout_xkey().cloned(),
    );

    let oidc = build_oidc_verifier(&env).await;
    let mtls = build_mtls_verifier(&env);

    if oidc.is_none() && mtls.is_none() {
        // A process that boots with zero verifiers looks healthy but
        // denies every callout — keep the failure mode loud and
        // consistent with the other required-config exits above.
        tracing::error!(
            "no credential verifiers configured; set AUTH_CALLOUT_OIDC_ISSUER and/or AUTH_CALLOUT_MTLS_TRUST_ANCHORS"
        );
        std::process::exit(1);
    }

    let tenants: Vec<&str> = admission_keys
        .tenant_signing_keys()
        .tenants()
        .map(TenantAccount::as_str)
        .collect();
    info!(
        nats_url = %nats.servers.join(","),
        auth_account = %admission_keys.auth_account(),
        tenants = ?tenants,
        "connecting to NATS for auth callout"
    );

    let client = trogon_nats::connect(&nats, NATS_CONNECT_TIMEOUT)
        .await
        .unwrap_or_else(|e| {
            tracing::error!(error = %e, "failed to connect to NATS");
            std::process::exit(1);
        });

    let dispatcher = CalloutDispatcher::new(CalloutDispatcherConfig {
        tenant_signing_keys: admission_keys.tenant_signing_keys().clone(),
        user_jwt_ttl,
        oidc,
        mtls,
        api_key: None,
    });
    let subscriber = Subscriber::new(client, dispatcher, wire);

    info!("auth callout subscriber running");

    // `Subscriber::run` returns `Ok(())` when the NATS subscription closes
    // (connection drop, shutdown, etc.) — that is never a healthy steady
    // state for this process, so exit non-zero in both arms so the
    // orchestrator restarts the pod instead of treating the silent exit
    // as a clean shutdown.
    match subscriber.run(&env).await {
        Ok(()) => {
            tracing::error!("auth callout subscriber exited cleanly; NATS subscription closed");
            std::process::exit(1);
        }
        Err(e) => {
            tracing::error!(error = %e, "auth callout subscriber exited with error");
            std::process::exit(1);
        }
    }
}
