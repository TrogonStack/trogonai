#![allow(clippy::unwrap_used)]
use mcp_postgres::{HttpTransportConfig, HttpTransportConfigInput};
fn input(resource_url: &str, origins: &[&str]) -> HttpTransportConfigInput {
    HttpTransportConfigInput {
        bind: "127.0.0.1:8080".parse().unwrap(),
        resource_url: resource_url.into(),
        allowed_origins: origins.iter().map(|origin| (*origin).into()).collect(),
    }
}
#[test]
fn public_resources_require_canonical_https_and_local_resources_allow_http() {
    for resource in [
        "https://sql.example.test/mcp",
        "http://127.0.0.1:8080/mcp",
        "http://localhost:8080/mcp",
        "http://[::1]:8080/mcp",
    ] {
        let config = HttpTransportConfig::new(input(resource, &[])).unwrap();
        assert_eq!(config.resource_url(), resource);
        assert_eq!(config.bind(), "127.0.0.1:8080".parse().unwrap());
    }
    for resource in [
        "http://sql.example.test/mcp",
        "https://sql.example.test/other",
        "https://sql.example.test/mcp/",
        "https://user@sql.example.test/mcp",
        "https://user:password@sql.example.test/mcp",
        "https://sql.example.test/mcp?query",
        "https://sql.example.test/mcp#fragment",
        "https://SQL.example.test/mcp",
        "invalid",
        "file:///mcp",
    ] {
        assert!(HttpTransportConfig::new(input(resource, &[])).is_err());
    }
}
#[test]
fn browser_origins_are_explicit_exact_origins() {
    for origin in [
        "https://app.example.test",
        "https://app.example.test:8443",
        "http://localhost:8080",
    ] {
        assert!(HttpTransportConfig::new(input("https://sql.example.test/mcp", &[origin])).is_ok());
    }
    for origin in [
        "*",
        "null",
        "https://app.example.test/",
        "https://app.example.test/path",
        "http://app.example.test",
        "https://app.example.test?x",
        "https://app.example.test#x",
        "https://user@app.example.test",
    ] {
        assert!(HttpTransportConfig::new(input("https://sql.example.test/mcp", &[origin])).is_err());
    }
}
