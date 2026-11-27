#[test]
fn browser_image_is_an_explicit_operator_option() -> anyhow::Result<()> {
    let args = <super::Args as clap::Parser>::try_parse_from([
        "trogon-sandbox-docker-worker",
        "--egress-proxy-image",
        "sandbox-egress-proxyd:dev",
        "--browser-image",
        "registry.example.test/sandbox/browser:node22",
    ])?;
    assert_eq!(
        args.browser_image.as_deref(),
        Some("registry.example.test/sandbox/browser:node22"),
        "browser image must be configurable"
    );
    Ok(())
}

#[test]
fn default_args_parse_successfully() -> anyhow::Result<()> {
    let args = <super::Args as clap::Parser>::try_parse_from([
        "trogon-sandbox-docker-worker",
        "--egress-proxy-image",
        "sandbox-egress-proxyd:dev",
    ])?;
    assert_eq!(args.nats_url, "nats://127.0.0.1:4222");
    assert_eq!(args.image, "sandbox-agent:dev");
    assert_eq!(args.max_concurrent_leases, 4);
    assert!(args.warm_pool_enabled);
    assert_eq!(args.warm_pool_max_per_digest, 3);
    assert_eq!(args.browser_image, None);
    Ok(())
}

#[test]
fn explicit_args_parse_successfully() -> anyhow::Result<()> {
    let args = <super::Args as clap::Parser>::try_parse_from([
        "trogon-sandbox-docker-worker",
        "--nats-url",
        "nats://broker:4222",
        "--image",
        "my-registry/agent:v1",
        "--max-concurrent-leases",
        "8",
        "--egress-proxy-image",
        "sandbox-egress-proxyd:dev",
    ])?;
    assert_eq!(args.nats_url, "nats://broker:4222");
    assert_eq!(args.image, "my-registry/agent:v1");
    assert_eq!(args.max_concurrent_leases, 8);
    Ok(())
}
