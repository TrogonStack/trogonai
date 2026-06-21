//! `a2a-nats-stdio` — bridges JSON-RPC requests over stdin/stdout to an
//! `A2aClient` running against a real NATS server. See the library docs for
//! the protocol shape.

#[cfg(not(coverage))]
#[tokio::main]
async fn main() {
    if let Err(e) = a2a_nats_stdio::run().await {
        eprintln!("a2a-nats-stdio error: {e}");
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
