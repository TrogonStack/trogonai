use clap::Parser;

use super::super::{ServeError, WorkerArgs};
use super::support::TestResult;

const IMAGE: &str = "registry.example/agent@sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

fn parse(extra: &[&str]) -> Result<WorkerArgs, clap::Error> {
    let mut args = vec![
        "worker",
        "--ate-api-endpoint",
        "http://ate-api-server:8080",
        "--agent-image",
        IMAGE,
    ];
    args.extend_from_slice(extra);
    WorkerArgs::try_parse_from(args)
}

const MTLS: [&str; 6] = [
    "--router-tls-ca-file",
    "ca.pem",
    "--router-tls-client-cert-file",
    "cert.pem",
    "--router-tls-client-key-file",
    "key.pem",
];

#[test]
fn the_default_size_classes_are_accepted() -> TestResult {
    let args = parse(&["--allow-insecure-router"])?;

    args.size_classes()?;
    Ok(())
}

#[test]
fn a_malformed_size_class_table_names_the_offending_value() -> TestResult {
    let args = parse(&["--allow-insecure-router", "--size-class", "small"])?;

    let error = args.size_classes().err().ok_or("accepted a malformed table")?;

    assert!(matches!(error, ServeError::SizeClass { ref value, .. } if value == "small"));
    Ok(())
}

#[test]
fn complete_router_material_selects_tls_for_the_endpoint_host_by_default() -> TestResult {
    let args = parse(&MTLS)?;

    assert!(args.router_tls()?.is_some());
    Ok(())
}

#[test]
fn an_explicit_router_server_name_is_used_for_tls() -> TestResult {
    let mut flags = MTLS.to_vec();
    flags.extend(["--router-tls-server-name", "router.internal"]);

    assert!(parse(&flags)?.router_tls()?.is_some());
    Ok(())
}

#[test]
fn an_invalid_router_server_name_is_refused() -> TestResult {
    let mut flags = MTLS.to_vec();
    flags.extend(["--router-tls-server-name", "not a host name"]);

    let error = parse(&flags)?.router_tls().err().ok_or("accepted a bad server name")?;

    assert!(matches!(error, ServeError::RouterTls(_)));
    Ok(())
}

#[test]
fn plaintext_routers_need_the_explicit_opt_in() -> TestResult {
    assert!(parse(&["--allow-insecure-router"])?.router_tls()?.is_none());
    assert!(matches!(
        parse(&[])?
            .router_tls()
            .err()
            .ok_or("accepted plaintext without the opt-in")?,
        ServeError::RouterTlsIncomplete
    ));
    Ok(())
}

#[test]
fn partial_router_material_is_refused_rather_than_downgraded() -> TestResult {
    let args = parse(&["--router-tls-ca-file", "ca.pem"])?;

    assert!(matches!(
        args.router_tls().err().ok_or("accepted partial material")?,
        ServeError::RouterTlsIncomplete
    ));
    Ok(())
}

#[test]
fn the_opt_in_conflicts_with_any_tls_material() {
    assert!(parse(&["--allow-insecure-router", "--router-tls-ca-file", "ca.pem"]).is_err());
}

#[test]
fn the_arguments_describe_a_valid_provider_configuration() -> TestResult {
    let config = parse(&["--allow-insecure-router", "--agent-port", "9000"])?.substrate_config()?;

    assert_eq!(config.agent_port, 9000);
    assert_eq!(config.agent_image, IMAGE);
    assert!(config.router_tls.is_none());
    Ok(())
}

#[test]
fn an_invalid_configuration_is_refused() -> TestResult {
    let args = parse(&["--allow-insecure-router", "--agent-port", "0"])?;

    assert!(matches!(
        args.substrate_config().err().ok_or("accepted port zero")?,
        ServeError::Config(_)
    ));
    Ok(())
}

#[test]
fn a_bad_size_class_or_router_setup_stops_the_configuration() -> TestResult {
    assert!(
        parse(&["--allow-insecure-router", "--size-class", "x"])?
            .substrate_config()
            .is_err()
    );
    assert!(parse(&[])?.substrate_config().is_err());
    Ok(())
}

#[test]
fn the_control_plane_connection_is_plaintext_without_a_ca_file() -> TestResult {
    let security = parse(&["--allow-insecure-router"])?.connect_security()?;

    assert!(security.tls.is_none());
    assert!(security.bearer_token_file.is_some());
    Ok(())
}

#[test]
fn a_ca_file_and_token_file_secure_the_control_plane_connection() -> TestResult {
    let dir = tempfile::tempdir()?;
    let ca = dir.path().join("ca.pem");
    std::fs::write(&ca, b"pem")?;
    let ca_flag = ca.to_str().ok_or("non-utf8 temp path")?;

    let security = parse(&[
        "--allow-insecure-router",
        "--tls-ca-file",
        ca_flag,
        "--tls-server-name",
        "localhost",
        "--bearer-token-file",
        "token",
    ])?
    .connect_security()?;

    assert!(security.tls.is_some());
    Ok(())
}

#[test]
fn an_unreadable_ca_file_names_its_path() -> TestResult {
    let dir = tempfile::tempdir()?;
    let missing = dir.path().join("missing.pem");
    let flag = missing.to_str().ok_or("non-utf8 temp path")?;

    let error = parse(&["--allow-insecure-router", "--tls-ca-file", flag])?
        .connect_security()
        .err()
        .ok_or("read a missing CA file")?;

    assert!(matches!(error, ServeError::TlsCaFile { ref path, .. } if path == &missing));
    Ok(())
}
