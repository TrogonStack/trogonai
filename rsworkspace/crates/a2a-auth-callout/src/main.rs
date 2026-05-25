use std::sync::Arc;
use std::time::Duration;

use tracing::{info, warn};

use a2a_auth_callout::credentials::mtls::{TrustAnchorPem, X509MtlsVerifier};
use a2a_auth_callout::credentials::oidc::{JwksOidcVerifier, OidcIssuerUrl, OidcVerifier};
use a2a_auth_callout::credentials::mtls::MTlsVerifier;
use a2a_auth_callout::dispatcher::{CalloutDispatcher, CalloutDispatcherConfig};
use a2a_auth_callout::error::AuthCalloutError;
use a2a_auth_callout::signing_key_source::{
    EnvSigningKeySource, FileSigningKeySource, SigningKeySource,
};
use a2a_auth_callout::{
    AccountResolver, AuthCalloutWireCodec, NkeyPublic, NkeySeed, StaticAccountResolver,
    Subscriber, XkeyPublic,
};

const DEFAULT_USER_JWT_TTL_SECS: u64 = 300;

fn split_env_list(name: &str) -> Vec<String> {
    std::env::var(name)
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

fn env_required(name: &str) -> Result<String, String> {
    std::env::var(name).map_err(|_| format!("{name} is required"))
}

fn load_nkey_seed_env(name: &str) -> Result<NkeySeed, String> {
    let raw = env_required(name)?;
    NkeySeed::parse(raw).map_err(|e| e.to_string())
}

fn load_nkey_public_env(name: &str) -> Result<NkeyPublic, String> {
    let raw = env_required(name)?;
    NkeyPublic::parse(raw).map_err(|e| e.to_string())
}

fn load_signing_key_source() -> Result<Arc<dyn SigningKeySource>, AuthCalloutError> {
    let kind = std::env::var("AUTH_CALLOUT_SIGNING_KEY_SOURCE")
        .unwrap_or_else(|_| "env".into());
    match kind.as_str() {
        "env" => {
            if std::env::var("AUTH_CALLOUT_SIGNING_SECRET").is_err() {
                let fallback = std::env::var("AUTH_CALLOUT_ISSUER_NKEY_SEED").map_err(|_| {
                    AuthCalloutError::Internal(
                        "AUTH_CALLOUT_SIGNING_SECRET or AUTH_CALLOUT_ISSUER_NKEY_SEED is required for env custody"
                            .into(),
                    )
                })?;
                unsafe {
                    std::env::set_var("AUTH_CALLOUT_SIGNING_SECRET", fallback);
                }
            }
            Ok(Arc::new(EnvSigningKeySource::from_env()?))
        }
        "file" => {
            let current = std::env::var("AUTH_CALLOUT_SIGNING_KEY_PATH").map_err(|_| {
                AuthCalloutError::Internal(
                    "AUTH_CALLOUT_SIGNING_KEY_PATH is required when AUTH_CALLOUT_SIGNING_KEY_SOURCE=file"
                        .into(),
                )
            })?;
            let previous = std::env::var("AUTH_CALLOUT_SIGNING_KEY_PREVIOUS_PATH").ok();
            Ok(Arc::new(FileSigningKeySource::new(
                current,
                previous.as_deref(),
            )?))
        }
        "vault" => {
            Err(AuthCalloutError::Internal(
                "AUTH_CALLOUT_SIGNING_KEY_SOURCE=vault is not wired yet; use file or env"
                    .into(),
            ))
        }
        other => Err(AuthCalloutError::Internal(format!(
            "unknown AUTH_CALLOUT_SIGNING_KEY_SOURCE: {other} (expected env, file, or vault)"
        ))),
    }
}

async fn build_oidc_verifier() -> Option<Arc<dyn OidcVerifier>> {
    let issuer_raw = std::env::var("AUTH_CALLOUT_OIDC_ISSUER").ok()?;
    let issuer = match OidcIssuerUrl::parse(&issuer_raw) {
        Ok(i) => i,
        Err(e) => {
            warn!(error = %e, "AUTH_CALLOUT_OIDC_ISSUER set but invalid; OIDC disabled");
            return None;
        }
    };
    let audiences = split_env_list("AUTH_CALLOUT_OIDC_AUDIENCES");
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

fn build_mtls_verifier() -> Option<Arc<dyn MTlsVerifier>> {
    let path = std::env::var("AUTH_CALLOUT_MTLS_TRUST_ANCHORS").ok()?;
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
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let nats_url = std::env::var("NATS_URL").unwrap_or_else(|_| "nats://localhost:4222".into());
    let signing_key_source = match load_signing_key_source() {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(error = %e, "failed to load signing key custody");
            std::process::exit(1);
        }
    };
    let allowed_accounts = split_env_list("AUTH_CALLOUT_ALLOWED_ACCOUNTS");
    if allowed_accounts.is_empty() {
        tracing::error!(
            "AUTH_CALLOUT_ALLOWED_ACCOUNTS must list at least one tenant account"
        );
        std::process::exit(1);
    }
    let user_jwt_ttl = std::env::var("AUTH_CALLOUT_USER_JWT_TTL_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or_else(|| Duration::from_secs(DEFAULT_USER_JWT_TTL_SECS));

    let server_issuer = match load_nkey_public_env("AUTH_CALLOUT_SERVER_NKEY_PUBLIC") {
        Ok(k) => k,
        Err(e) => {
            tracing::error!(error = %e, "invalid server NKey configuration");
            std::process::exit(1);
        }
    };
    let callout_issuer_seed = match load_nkey_seed_env("AUTH_CALLOUT_ISSUER_NKEY_SEED") {
        Ok(k) => k,
        Err(e) => {
            tracing::error!(error = %e, "invalid callout issuer NKey seed");
            std::process::exit(1);
        }
    };
    let account_xkey_seed = std::env::var("AUTH_CALLOUT_XKEY_SEED")
        .ok()
        .map(NkeySeed::parse)
        .transpose()
        .map_err(|e| e.to_string());
    let account_xkey_seed = match account_xkey_seed {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(error = %e, "invalid AUTH_CALLOUT_XKEY_SEED");
            std::process::exit(1);
        }
    };

    let server_xkey_public = std::env::var("AUTH_CALLOUT_SERVER_XKEY_PUBLIC")
        .ok()
        .map(XkeyPublic::parse)
        .transpose()
        .map_err(|e| e.to_string());
    let server_xkey_public = match server_xkey_public {
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
    let oidc = build_oidc_verifier().await;
    let mtls = build_mtls_verifier();

    if oidc.is_none() && mtls.is_none() {
        warn!(
            "no credential verifiers configured; all dispatch attempts will be denied"
        );
    }

    info!(nats_url = %nats_url, accounts = ?allowed_accounts, "connecting to NATS for auth callout");

    let client = async_nats::connect(&nats_url).await.unwrap_or_else(|e| {
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

    if let Err(e) = subscriber.run().await {
        tracing::error!(error = %e, "auth callout subscriber exited with error");
        std::process::exit(1);
    }
}
