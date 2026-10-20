#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::{io, sync::Arc};

use async_trait::async_trait;
use mcp_postgres::{ConfigInput, Error, PermissionCheck, PermissionRequest, QueryService, RuntimeConfig};
use rcgen::{BasicConstraints, CertificateParams, DnType, IsCa, Issuer, KeyPair};
use rustls::{CertificateError, Error as TlsError};
use serde_json::json;
use sqlx::PgPool;
use tempfile::TempDir;
use testcontainers_modules::{
    postgres::Postgres,
    testcontainers::{ContainerAsync, ImageExt, runners::AsyncRunner},
};
use url::Url;

struct Permissions;

#[async_trait]
impl PermissionCheck for Permissions {
    async fn check_all(&self, _: &[PermissionRequest]) -> Result<(), Error> {
        Ok(())
    }
}

fn authority(name: &str) -> (CertificateParams, KeyPair) {
    let mut params = CertificateParams::default();
    params.distinguished_name.push(DnType::CommonName, name);
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    (params, KeyPair::generate().unwrap())
}

struct Fixture {
    _container: ContainerAsync<Postgres>,
    certificates: TempDir,
    admin: PgPool,
    port: u16,
}

impl Fixture {
    async fn new() -> Self {
        let certificates = tempfile::tempdir().unwrap();
        let (ca_params, ca_key) = authority("trusted-test-ca");
        let ca = ca_params.self_signed(&ca_key).unwrap();
        std::fs::write(certificates.path().join("ca.pem"), ca.pem()).unwrap();
        let (other_params, other_key) = authority("untrusted-test-ca");
        let other_ca = other_params.self_signed(&other_key).unwrap();
        std::fs::write(certificates.path().join("other-ca.pem"), other_ca.pem()).unwrap();
        let server_key = KeyPair::generate().unwrap();
        let server = CertificateParams::new(vec!["localhost".into()])
            .unwrap()
            .signed_by(&server_key, &Issuer::from_params(&ca_params, &ca_key))
            .unwrap();
        let container = Postgres::default()
            .with_tag("18.3-alpine")
            .with_copy_to("/tmp/server.crt", server.pem().into_bytes())
            .with_copy_to("/tmp/server.key", server_key.serialize_pem().into_bytes())
            .with_cmd([
                "sh",
                "-c",
                "chown postgres:postgres /tmp/server.key && chmod 600 /tmp/server.key && exec docker-entrypoint.sh postgres -c ssl=on -c ssl_cert_file=/tmp/server.crt -c ssl_key_file=/tmp/server.key",
            ])
            .start()
            .await
            .unwrap();
        let port = container.get_host_port_ipv4(5432).await.unwrap();
        let host = container.get_host().await.unwrap();
        let admin = PgPool::connect(&format!(
            "postgres://postgres:postgres@{host}:{port}/postgres?sslmode=disable"
        ))
        .await
        .unwrap();
        sqlx::raw_sql(
            "CREATE ROLE query_role LOGIN PASSWORD 'query-only';
             REVOKE TEMPORARY ON DATABASE postgres FROM PUBLIC;
             CREATE TABLE public.customers (id integer);
             INSERT INTO public.customers VALUES (1);
             GRANT USAGE ON SCHEMA public TO query_role;
             GRANT SELECT ON public.customers TO query_role;",
        )
        .execute(&admin)
        .await
        .unwrap();
        Self {
            _container: container,
            certificates,
            admin,
            port,
        }
    }

    fn dsn(&self, host: &str, ca: &str) -> String {
        let mut dsn = Url::parse(&format!(
            "postgres://query_role:query-only@{host}:{}/postgres",
            self.port
        ))
        .unwrap();
        dsn.query_pairs_mut()
            .append_pair("sslmode", "verify-full")
            .append_pair("sslrootcert", self.certificates.path().join(ca).to_str().unwrap())
            .append_pair("application_name", "mcp-postgres-tls-test");
        dsn.into()
    }

    async fn trusted_service(&self) -> QueryService {
        service(&self.dsn("localhost", "ca.pem")).await.unwrap()
    }
}

async fn service(dsn: &str) -> Result<QueryService, Error> {
    let input: ConfigInput = toml::from_str(include_str!("../examples/config.toml")).unwrap();
    let config = RuntimeConfig::from_input(input, |key| match key {
        "MCP_POSTGRES_SPICEDB_TOKEN" => Some("test-only-token".into()),
        "MCP_POSTGRES_ANALYTICS_DSN" => Some(dsn.to_owned()),
        _ => None,
    })
    .unwrap();
    QueryService::connect(config, Arc::new(Permissions)).await
}

fn certificate_error(result: Result<QueryService, Error>) -> CertificateError {
    let Err(Error::Database(sqlx::Error::Io(error))) = result else {
        panic!("expected a TLS certificate validation failure");
    };
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    let Some(TlsError::InvalidCertificate(error)) = error.get_ref().and_then(|error| error.downcast_ref::<TlsError>())
    else {
        panic!("expected a typed certificate validation cause");
    };
    error.clone()
}

#[tokio::test]
async fn trusted_ca_and_matching_hostname_protect_the_query_connection() {
    let fixture = Fixture::new().await;
    let service = fixture.trusted_service().await;
    let encrypted: bool = sqlx::query_scalar(
        "SELECT count(*) > 0 AND bool_and(s.ssl) FROM pg_catalog.pg_stat_activity a
         JOIN pg_catalog.pg_stat_ssl s USING (pid)
         WHERE a.usename='query_role' AND a.application_name='mcp-postgres-tls-test'",
    )
    .fetch_one(&fixture.admin)
    .await
    .unwrap();
    assert!(encrypted);
    let result = service
        .query(&identity(), "analytics-readonly", "SELECT id FROM public.customers")
        .await
        .unwrap();
    assert_eq!(result.rows, vec![vec![json!(1)]]);
}

#[tokio::test]
async fn an_untrusted_ca_is_rejected_by_the_query_service() {
    let fixture = Fixture::new().await;
    let _trusted = fixture.trusted_service().await;
    let error = certificate_error(service(&fixture.dsn("localhost", "other-ca.pem")).await);
    assert!(matches!(error, CertificateError::UnknownIssuer));
}

#[tokio::test]
async fn a_trusted_certificate_for_another_hostname_is_rejected() {
    let fixture = Fixture::new().await;
    let _trusted = fixture.trusted_service().await;
    let error = certificate_error(service(&fixture.dsn("127.0.0.1", "ca.pem")).await);
    assert!(matches!(error, CertificateError::NotValidForNameContext { .. }));
}

#[tokio::test]
async fn verify_full_refuses_a_server_without_tls() {
    let container = Postgres::default().with_tag("18.3-alpine").start().await.unwrap();
    let host = container.get_host().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let dsn = format!("postgres://postgres:postgres@{host}:{port}/postgres");
    let admin = PgPool::connect(&format!("{dsn}?sslmode=disable")).await.unwrap();
    let tls_enabled: String = sqlx::query_scalar("SHOW ssl").fetch_one(&admin).await.unwrap();
    assert_eq!(tls_enabled, "off");
    assert!(matches!(
        service(&format!("{dsn}?sslmode=verify-full")).await,
        Err(Error::Database(sqlx::Error::Tls(_)))
    ));
}

fn identity() -> mcp_postgres::RequestIdentity {
    mcp_postgres::RequestIdentity::new(
        mcp_postgres::ObjectId::new("example-user".into()).unwrap(),
        mcp_postgres::ObjectId::new("example-client".into()).unwrap(),
        Vec::new(),
    )
}
