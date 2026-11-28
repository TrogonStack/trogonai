use clap::Parser;
use trogon_sandbox_ws_gateway::{GatewayArgs, TlsFiles};

fn parse(extra: &[&str]) -> Result<GatewayArgs, clap::Error> {
    let mut flags = vec!["gateway", "--nats-url", "nats://127.0.0.1:4222"];
    flags.extend_from_slice(extra);
    GatewayArgs::try_parse_from(flags)
}

#[test]
fn no_tls_flags_serve_plain_http() {
    assert_eq!(parse(&[]).unwrap().tls.files(), None);
}

#[test]
fn both_flags_form_one_tls_identity() {
    let args = parse(&["--tls-cert", "/etc/tls/cert.pem", "--tls-key", "/etc/tls/key.pem"]).unwrap();
    assert_eq!(
        args.tls.files(),
        Some(TlsFiles::new("/etc/tls/cert.pem", "/etc/tls/key.pem"))
    );
}

#[test]
fn a_lone_certificate_is_a_parse_error() {
    let error = parse(&["--tls-cert", "/etc/tls/cert.pem"]).unwrap_err();
    assert_eq!(error.kind(), clap::error::ErrorKind::MissingRequiredArgument);
}

#[test]
fn a_lone_key_is_a_parse_error() {
    let error = parse(&["--tls-key", "/etc/tls/key.pem"]).unwrap_err();
    assert_eq!(error.kind(), clap::error::ErrorKind::MissingRequiredArgument);
}
