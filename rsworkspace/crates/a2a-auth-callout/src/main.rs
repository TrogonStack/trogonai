#[cfg(not(coverage))]
use {
    a2a_auth_callout::credentials::mtls::MTlsVerifier,
    a2a_auth_callout::credentials::mtls::{TrustAnchorPem, X509MtlsVerifier},
    a2a_auth_callout::credentials::oidc::{JwksOidcVerifier, OidcIssuerUrl, OidcVerifier},
    a2a_auth_callout::dispatcher::{CalloutDispatcher, CalloutDispatcherConfig},
    a2a_auth_callout::error::AuthCalloutError,
    a2a_auth_callout::signing_key_source::signing_key_source_from_process_env,
    a2a_auth_callout::{
        AccountResolver, AuthCalloutWireCodec, NkeyPublic, NkeySeed, StaticAccountResolver, Subscriber, XkeyPublic,
    },
    std::sync::Arc,
    std::time::Duration,
    tracing::{info, warn},
    trogon_std::env::{ReadEnv, SystemEnv},
};

#[cfg(not(coverage))]
const DEFAULT_USER_JWT_TTL_SECS: u64 = 300;

#[cfg(not(coverage))]
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

#[cfg(not(coverage))]
fn env_required(env: &impl ReadEnv, name: &'static str) -> Result<String, AuthCalloutError> {
    env.var(name).map_err(|_| AuthCalloutError::MissingEnvVar(name))
}

#[cfg(not(coverage))]
fn load_nkey_seed_env(env: &impl ReadEnv, name: &'static str) -> Result<NkeySeed, AuthCalloutError> {
    NkeySeed::parse(env_required(env, name)?)
}

#[cfg(not(coverage))]
fn load_nkey_public_env(env: &impl ReadEnv, name: &'static str) -> Result<NkeyPublic, AuthCalloutError> {
    NkeyPublic::parse(env_required(env, name)?)
}

#[cfg(not(coverage))]
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

#[cfg(not(coverage))]
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

#[cfg(not(coverage))]
#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let env = SystemEnv;

    let nats_url = env.var("NATS_URL").unwrap_or_else(|_| "nats://localhost:4222".into());
    let signing_key_source = match signing_key_source_from_process_env() {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(error = %e, "invalid signing key source configuration");
            std::process::exit(1);
        }
    };
    let allowed_accounts = split_env_list(&env, "AUTH_CALLOUT_ALLOWED_ACCOUNTS");
    if allowed_accounts.is_empty() {
        tracing::error!("AUTH_CALLOUT_ALLOWED_ACCOUNTS must list at least one tenant account");
        std::process::exit(1);
    }
    let user_jwt_ttl = match env.var("AUTH_CALLOUT_USER_JWT_TTL_SECS") {
        Ok(raw) => match raw.parse::<u64>() {
            Ok(secs) => Duration::from_secs(secs),
            Err(e) => {
                tracing::error!(error = %e, raw = %raw, "invalid AUTH_CALLOUT_USER_JWT_TTL_SECS");
                std::process::exit(1);
            }
        },
        Err(_) => Duration::from_secs(DEFAULT_USER_JWT_TTL_SECS),
    };

    let server_issuer = match load_nkey_public_env(&env, "AUTH_CALLOUT_SERVER_NKEY_PUBLIC") {
        Ok(k) => k,
        Err(e) => {
            tracing::error!(error = %e, "invalid server NKey configuration");
            std::process::exit(1);
        }
    };
    let callout_issuer_seed = match load_nkey_seed_env(&env, "AUTH_CALLOUT_ISSUER_NKEY_SEED") {
        Ok(k) => k,
        Err(e) => {
            tracing::error!(error = %e, "invalid callout issuer NKey seed");
            std::process::exit(1);
        }
    };
    let account_xkey_seed = match env.var("AUTH_CALLOUT_XKEY_SEED").ok().map(NkeySeed::parse).transpose() {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(error = %e, "invalid AUTH_CALLOUT_XKEY_SEED");
            std::process::exit(1);
        }
    };

    let server_xkey_public = match env
        .var("AUTH_CALLOUT_SERVER_XKEY_PUBLIC")
        .ok()
        .map(XkeyPublic::parse)
        .transpose()
    {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(error = %e, "invalid AUTH_CALLOUT_SERVER_XKEY_PUBLIC");
            std::process::exit(1);
        }
    };

    let wire = match AuthCalloutWireCodec::new(
        server_issuer,
        callout_issuer_seed,
        account_xkey_seed,
        server_xkey_public,
    ) {
        Ok(w) => w,
        Err(e) => {
            tracing::error!(error = %e, "failed to build auth callout wire codec");
            std::process::exit(1);
        }
    };

    let resolver: Arc<dyn AccountResolver> = Arc::new(StaticAccountResolver::new(allowed_accounts.clone()));
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

    info!(nats_url = %nats_url, accounts = ?allowed_accounts, "connecting to NATS for auth callout");

    let connect_opts = match (env.var("NATS_USER").ok(), env.var("NATS_PASSWORD").ok()) {
        (Some(user), Some(password)) => async_nats::ConnectOptions::new().user_and_password(user, password),
        _ => async_nats::ConnectOptions::new(),
    };
    let client = connect_opts.connect(&nats_url).await.unwrap_or_else(|e| {
        tracing::error!(error = %e, "failed to connect to NATS");
        std::process::exit(1);
    });

    let dispatcher = CalloutDispatcher::new(CalloutDispatcherConfig {
        signing_key_source,
        user_jwt_ttl,
        account_resolver: resolver,
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

#[cfg(coverage)]
fn main() {}
