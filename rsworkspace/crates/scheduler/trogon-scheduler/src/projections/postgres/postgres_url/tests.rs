use sqlx::postgres::PgSslMode;
use trogon_std::env::InMemoryEnv;

use super::*;

const BASE: &str = "postgres://scheduler:hunter2@db.internal:5433/schedules";

fn parse(raw: &str) -> Result<PostgresUrl, PostgresUrlError> {
    PostgresUrl::parse(raw, &InMemoryEnv::new())
}

#[test]
fn verifies_the_server_when_no_sslmode_is_configured() {
    let url = parse(BASE).unwrap();

    assert_eq!(url.tls(), PostgresTls::VerifyFull);
    assert!(matches!(url.connect_options().get_ssl_mode(), PgSslMode::VerifyFull));
}

#[test]
fn keeps_the_connection_target() {
    let url = parse(BASE).unwrap();
    let options = url.connect_options();

    assert_eq!(options.get_host(), "db.internal");
    assert_eq!(options.get_port(), 5433);
    assert_eq!(options.get_username(), "scheduler");
    assert_eq!(options.get_database(), Some("schedules"));
}

#[test]
fn disables_tls_only_when_asked_explicitly() {
    let url = parse(&format!("{BASE}?sslmode=disable")).unwrap();

    assert_eq!(url.tls(), PostgresTls::Disabled);
    assert!(matches!(url.connect_options().get_ssl_mode(), PgSslMode::Disable));
}

#[test]
fn accepts_an_explicit_verify_full() {
    let url = parse(&format!("{BASE}?sslmode=verify-full")).unwrap();

    assert_eq!(url.tls(), PostgresTls::VerifyFull);
}

#[test]
fn accepts_the_ssl_mode_alias() {
    let url = parse(&format!("{BASE}?ssl-mode=disable")).unwrap();

    assert_eq!(url.tls(), PostgresTls::Disabled);
}

#[test]
fn uses_the_last_sslmode_like_the_driver_does() {
    let url = parse(&format!("{BASE}?sslmode=disable&sslmode=verify-full")).unwrap();

    assert_eq!(url.tls(), PostgresTls::VerifyFull);
    assert!(matches!(url.connect_options().get_ssl_mode(), PgSslMode::VerifyFull));
}

#[test]
fn rejects_sslmodes_that_do_not_fully_verify_the_server() {
    for mode in ["allow", "prefer", "require", "verify-ca"] {
        let error = parse(&format!("{BASE}?sslmode={mode}")).unwrap_err();

        assert!(
            matches!(error, PostgresUrlError::SslMode(PostgresTlsError::Unverified(_))),
            "{mode}: {error}"
        );
    }
}

#[test]
fn rejects_an_unknown_sslmode() {
    let error = parse(&format!("{BASE}?sslmode=on")).unwrap_err();

    assert!(
        matches!(error, PostgresUrlError::SslMode(PostgresTlsError::Unknown(_))),
        "{error}"
    );
}

#[test]
fn falls_back_to_pgsslmode() {
    let env = InMemoryEnv::new();
    env.set("PGSSLMODE", "disable");

    let url = PostgresUrl::parse(BASE, &env).unwrap();

    assert_eq!(url.tls(), PostgresTls::Disabled);
}

#[test]
fn prefers_the_url_sslmode_over_pgsslmode() {
    let env = InMemoryEnv::new();
    env.set("PGSSLMODE", "disable");

    let url = PostgresUrl::parse(&format!("{BASE}?sslmode=verify-full"), &env).unwrap();

    assert_eq!(url.tls(), PostgresTls::VerifyFull);
}

#[test]
fn rejects_a_pgsslmode_that_does_not_fully_verify_the_server() {
    let env = InMemoryEnv::new();
    env.set("PGSSLMODE", "require");

    let error = PostgresUrl::parse(BASE, &env).unwrap_err();

    assert!(
        matches!(error, PostgresUrlError::EnvSslMode(PostgresTlsError::Unverified(_))),
        "{error}"
    );
    assert_eq!(
        error.to_string(),
        format!("PGSSLMODE: {}", PostgresTlsError::Unverified(PgSslMode::Require))
    );
}

#[test]
fn ignores_an_empty_pgsslmode() {
    let env = InMemoryEnv::new();
    env.set("PGSSLMODE", "");

    let url = PostgresUrl::parse(BASE, &env).unwrap();

    assert_eq!(url.tls(), PostgresTls::VerifyFull);
}

#[test]
fn trusts_the_ca_named_by_pgsslrootcert() {
    let env = InMemoryEnv::new();
    env.set("PGSSLROOTCERT", "/etc/scheduler/postgres-ca.pem");

    let url = PostgresUrl::parse(BASE, &env).unwrap();

    assert_eq!(url.root_cert(), Some(Path::new("/etc/scheduler/postgres-ca.pem")));
}

#[test]
fn prefers_the_url_sslrootcert_over_pgsslrootcert() {
    let env = InMemoryEnv::new();
    env.set("PGSSLROOTCERT", "/etc/scheduler/postgres-ca.pem");

    let url = PostgresUrl::parse(&format!("{BASE}?sslrootcert=/run/secrets/ca.pem"), &env).unwrap();

    assert_eq!(url.root_cert(), Some(Path::new("/run/secrets/ca.pem")));
}

#[test]
fn trusts_no_extra_ca_unless_one_is_configured() {
    assert_eq!(parse(BASE).unwrap().root_cert(), None);
}

#[test]
fn accepts_both_postgres_schemes() {
    let url = parse("postgresql://scheduler@db.internal/schedules").unwrap();

    assert_eq!(url.tls(), PostgresTls::VerifyFull);
}

#[test]
fn rejects_other_schemes() {
    let error = parse("mysql://scheduler@db.internal/schedules").unwrap_err();

    assert!(matches!(error, PostgresUrlError::UnsupportedScheme(ref scheme) if scheme == "mysql"));
}

#[test]
fn malformed_url_error_does_not_repeat_the_password() {
    let error = parse("postgres://scheduler:hunter2@db.internal:notaport/schedules").unwrap_err();

    assert!(matches!(error, PostgresUrlError::Malformed(_)));
    assert_eq!(
        error.to_string(),
        format!("Postgres URL is malformed: {}", url::ParseError::InvalidPort)
    );
}

#[test]
fn debug_shows_the_target_but_not_the_password() {
    let debug = format!("{:?}", parse(BASE).unwrap());

    assert!(!debug.contains("hunter2"), "{debug}");
    assert!(debug.contains("db.internal"), "{debug}");
    assert!(debug.contains("VerifyFull"), "{debug}");
}
