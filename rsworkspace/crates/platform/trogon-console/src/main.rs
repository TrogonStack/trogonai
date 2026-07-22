#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "trogon_console=info".parse().unwrap()),
        )
        .init();

    if let Err(e) = trogon_console::run().await {
        eprintln!("trogon-console error: {e}");
        std::process::exit(1);
    }
}
