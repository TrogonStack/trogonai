#[tokio::main]
async fn main() {
    if let Err(e) = a2a_nats_stdio::run().await {
        eprintln!("a2a-nats-stdio error: {e}");
        std::process::exit(1);
    }
}
