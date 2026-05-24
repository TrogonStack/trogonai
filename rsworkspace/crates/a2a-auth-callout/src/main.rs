use std::sync::Arc;
use std::time::Duration;

use tracing::{info, warn};

use a2a_auth_callout::credentials::mtls::{TrustAnchorPem, X509MtlsVerifier};
use a2a_auth_callout::credentials::oidc::{JwksOidcVerifier, OidcIssuerUrl, OidcVerifier};
use a2a_auth_callout::credentials::mtls::MTlsVerifier;
use a2a_auth_callout::dispatcher::{CalloutDispatcher, CalloutDispatcherConfig};
use a2a_auth_callout::{
    AccountResolver, CalloutIssuer, DenialPublisherConfig, SigningKey, StaticAccountResolver,
    Subscriber,
};

const DEFAULT_CALLOUT_ISSUER: &str = "AUTH_CALLOUT_DEV_ISSUER";

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
    let signing_key_secret = std::env::var("AUTH_CALLOUT_SIGNING_SECRET")
        .unwrap_or_else(|_| "dev-secret-not-for-production".into());
    let callout_issuer_raw = std::env::var("AUTH_CALLOUT_ISSUER")
        .unwrap_or_else(|_| DEFAULT_CALLOUT_ISSUER.into());
    let callout_issuer = match CalloutIssuer::new(callout_issuer_raw) {
        Ok(i) => i,
        Err(e) => {
            tracing::error!(error = %e, "AUTH_CALLOUT_ISSUER is invalid");
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
        signing_key: SigningKey::from_secret(signing_key_secret.as_bytes()),
        user_jwt_ttl,
        account_resolver: resolver,
        oidc,
        mtls,
        api_key: None,
    });
    let denial = DenialPublisherConfig::new(
        SigningKey::from_secret(signing_key_secret.as_bytes()),
        callout_issuer,
    );
    let subscriber = Subscriber::new(client, dispatcher, denial);

    info!("auth callout subscriber running");

    if let Err(e) = subscriber.run().await {
        tracing::error!(error = %e, "auth callout subscriber exited with error");
        std::process::exit(1);
    }
}
