use tracing::info;

use a2a_auth_callout::{AuthCalloutError, AuthCalloutRequest, AuthCalloutResponse, AuthDispatcher, SigningKey, Subscriber};

struct StubDispatcher {
    #[allow(dead_code)]
    signing_key: SigningKey,
}

#[async_trait::async_trait]
impl AuthDispatcher for StubDispatcher {
    async fn dispatch(&self, _request: AuthCalloutRequest) -> Result<AuthCalloutResponse, AuthCalloutError> {
        unimplemented!("auth callout verify paths not implemented yet")
    }
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
    let signing_key_secret =
        std::env::var("AUTH_CALLOUT_SIGNING_SECRET").unwrap_or_else(|_| "dev-secret-not-for-production".into());

    info!(nats_url = %nats_url, "connecting to NATS for auth callout");

    let client = async_nats::connect(&nats_url).await.unwrap_or_else(|e| {
        tracing::error!(error = %e, "failed to connect to NATS");
        std::process::exit(1);
    });

    let signing_key = SigningKey::from_secret(signing_key_secret.as_bytes());
    let dispatcher = StubDispatcher { signing_key };
    let subscriber = Subscriber::new(client, dispatcher);

    info!("auth callout subscriber running; OIDC/mTLS verify paths are stubs");

    if let Err(e) = subscriber.run().await {
        tracing::error!(error = %e, "auth callout subscriber exited with error");
        std::process::exit(1);
    }
}
