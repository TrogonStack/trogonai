use clap::Parser;
use trogon_sandbox_auth::ApiKeyBucket;
use trogon_sandbox_ssh_gateway::GatewayArgs;

const NATS: [&str; 3] = ["trogon-sandbox-ssh-gateway", "--nats-url", "nats://127.0.0.1:4222"];

fn parse(extra: &[&str]) -> Result<GatewayArgs, clap::Error> {
    GatewayArgs::try_parse_from(NATS.iter().chain(extra))
}

#[test]
fn an_explicit_api_key_bucket_is_selected() -> Result<(), Box<dyn std::error::Error>> {
    let args = parse(&["--api-key-bucket", "tenant_keys-2"])?;
    assert_eq!(args.api_key_bucket, ApiKeyBucket::new("tenant_keys-2")?);
    Ok(())
}

#[test]
fn an_invalid_api_key_bucket_is_rejected() {
    for bucket in ["", "has space", "dot.ted", "slash/ed"] {
        assert!(parse(&["--api-key-bucket", bucket]).is_err(), "accepted {bucket:?}");
    }
}
