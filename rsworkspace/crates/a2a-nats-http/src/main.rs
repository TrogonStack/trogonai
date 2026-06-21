#[cfg(not(coverage))]
#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    if let Err(e) = a2a_nats_http::run().await {
        tracing::error!(error = %e, "A2A NATS HTTP server failed");
        std::process::exit(1);
    }
}

#[cfg(coverage)]
fn main() {}

#[cfg(test)]
mod tests {
    #[test]
    #[cfg(coverage)]
    fn coverage_main_stub_is_callable() {
        super::main();
    }
}
