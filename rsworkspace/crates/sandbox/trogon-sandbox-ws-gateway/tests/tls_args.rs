use clap::Parser;
use clap::error::ErrorKind;
use trogon_sandbox_ws_gateway::{GatewayArgs, TlsFiles};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn parse(extra: &[&str]) -> Result<GatewayArgs, clap::Error> {
    let mut flags = vec!["gateway", "--nats-url", "nats://127.0.0.1:4222"];
    flags.extend_from_slice(extra);
    GatewayArgs::try_parse_from(flags)
}

fn error_kind(extra: &[&str]) -> Option<ErrorKind> {
    parse(extra).err().map(|error| error.kind())
}

#[test]
fn no_tls_flags_serve_plain_http() -> TestResult {
    assert_eq!(parse(&[])?.tls.files(), None);
    Ok(())
}

#[test]
fn both_flags_form_one_tls_identity() -> TestResult {
    let args = parse(&["--tls-cert", "/etc/tls/cert.pem", "--tls-key", "/etc/tls/key.pem"])?;
    assert_eq!(
        args.tls.files(),
        Some(TlsFiles::new("/etc/tls/cert.pem", "/etc/tls/key.pem"))
    );
    Ok(())
}

#[test]
fn a_lone_certificate_is_a_parse_error() {
    assert_eq!(
        error_kind(&["--tls-cert", "/etc/tls/cert.pem"]),
        Some(ErrorKind::MissingRequiredArgument)
    );
}

#[test]
fn a_lone_key_is_a_parse_error() {
    assert_eq!(
        error_kind(&["--tls-key", "/etc/tls/key.pem"]),
        Some(ErrorKind::MissingRequiredArgument)
    );
}
