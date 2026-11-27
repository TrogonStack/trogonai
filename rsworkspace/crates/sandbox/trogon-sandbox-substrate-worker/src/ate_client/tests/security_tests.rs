use std::error::Error;
use std::time::Duration;

use rcgen::{CertifiedKey, generate_simple_self_signed};
use tempfile::TempDir;
use tonic::Code;

use super::fake_ate::{FakeAte, Served};
use crate::ate_client::{AteError, BearerTokenFile, ConnectError, ConnectSecurity, Control, TlsTrust, TonicControl};
use crate::proto;

type TestResult = Result<(), Box<dyn Error>>;

const TIMEOUT: Duration = Duration::from_secs(5);

fn reference() -> proto::ObjectRef {
    proto::ObjectRef {
        atespace: "tenant".into(),
        name: "actor".into(),
    }
}

async fn connect_with_token(served: &Served, token_file: BearerTokenFile) -> Result<TonicControl, ConnectError> {
    let security = ConnectSecurity {
        tls: None,
        bearer_token_file: Some(token_file),
    };
    TonicControl::connect(&served.http_endpoint(), TIMEOUT, security).await
}

#[tokio::test]
async fn no_token_file_sends_no_authorization() -> TestResult {
    let served = Served::plaintext(FakeAte::default()).await?;
    served.client().await?.get_actor(reference()).await?;
    assert_eq!(served.fake.calls()[0].authorization, None);
    Ok(())
}

#[tokio::test]
async fn the_token_is_trimmed_and_sent_as_a_bearer_credential() -> TestResult {
    let dir = TempDir::new()?;
    let path = dir.path().join("token");
    std::fs::write(&path, "secret-token\n")?;
    let served = Served::plaintext(FakeAte::default()).await?;
    let client = connect_with_token(&served, BearerTokenFile::new(&path)).await?;
    client.get_actor(reference()).await?;
    assert_eq!(
        served.fake.calls()[0].authorization.as_deref(),
        Some("Bearer secret-token")
    );
    Ok(())
}

#[tokio::test]
async fn a_rotated_token_is_picked_up_without_reconnecting() -> TestResult {
    let dir = TempDir::new()?;
    let path = dir.path().join("token");
    std::fs::write(&path, "first")?;
    let served = Served::plaintext(FakeAte::default()).await?;
    let client = connect_with_token(&served, BearerTokenFile::new(&path)).await?;
    client.get_actor(reference()).await?;
    std::fs::write(&path, "second")?;
    client.get_actor(reference()).await?;

    let tokens: Vec<_> = served.fake.calls().into_iter().map(|call| call.authorization).collect();
    assert_eq!(
        tokens,
        [Some("Bearer first".to_string()), Some("Bearer second".to_string())]
    );
    Ok(())
}

#[tokio::test]
async fn an_unreadable_token_file_is_an_identity_fault_and_never_reaches_the_server() -> TestResult {
    let dir = TempDir::new()?;
    let served = Served::plaintext(FakeAte::default()).await?;
    let client = connect_with_token(&served, BearerTokenFile::new(dir.path().join("missing"))).await?;
    let err = client.get_actor(reference()).await.err().ok_or("expected an error")?;
    assert!(matches!(err, AteError::PermissionDenied { .. }), "{err}");
    assert!(err.message().contains("reading bearer token"), "{err}");
    assert!(served.fake.calls().is_empty());
    Ok(())
}

#[tokio::test]
async fn a_token_that_cannot_be_header_metadata_is_an_identity_fault() -> TestResult {
    let dir = TempDir::new()?;
    let path = dir.path().join("token");
    std::fs::write(&path, "line one\nline two")?;
    let served = Served::plaintext(FakeAte::default()).await?;
    let client = connect_with_token(&served, BearerTokenFile::new(&path)).await?;
    let err = client.get_actor(reference()).await.err().ok_or("expected an error")?;
    assert!(matches!(err, AteError::PermissionDenied { .. }), "{err}");
    assert!(err.message().contains("not valid header metadata"), "{err}");
    Ok(())
}

#[test]
fn the_default_token_is_the_projected_service_account_token() {
    assert!(
        format!("{:?}", BearerTokenFile::default_service_account())
            .contains("/var/run/secrets/kubernetes.io/serviceaccount/token")
    );
}

struct Pki {
    certificate: String,
    key: String,
    dir: TempDir,
}

impl Pki {
    fn new() -> Result<Self, Box<dyn Error>> {
        // Both TLS backends are linked into the test binary, so rustls cannot pick one on
        // its own. Installing twice is harmless; the second attempt reports the first.
        let _ = tokio_rustls::rustls::crypto::aws_lc_rs::default_provider().install_default();
        let CertifiedKey { cert, signing_key } = generate_simple_self_signed(vec!["localhost".to_string()])?;
        Ok(Self {
            certificate: cert.pem(),
            key: signing_key.serialize_pem(),
            dir: TempDir::new()?,
        })
    }

    fn trust(&self, server_name: Option<&str>) -> Result<TlsTrust, Box<dyn Error>> {
        let path = self.dir.path().join("ca.pem");
        std::fs::write(&path, &self.certificate)?;
        Ok(TlsTrust::from_ca_file(path, server_name.map(str::to_string))?)
    }
}

#[tokio::test]
async fn tls_connects_when_the_server_name_override_matches_the_certificate() -> TestResult {
    let pki = Pki::new()?;
    let served = Served::tls(FakeAte::default(), &pki.certificate, &pki.key).await?;
    let security = ConnectSecurity {
        tls: Some(pki.trust(Some("localhost"))?),
        bearer_token_file: None,
    };
    let client = TonicControl::connect(&served.https_endpoint(), TIMEOUT, security).await?;
    client.get_actor(reference()).await?;
    assert_eq!(served.fake.calls().len(), 1);
    Ok(())
}

#[tokio::test]
async fn tls_checks_the_endpoint_host_when_no_server_name_is_given() -> TestResult {
    let pki = Pki::new()?;
    let served = Served::tls(FakeAte::default(), &pki.certificate, &pki.key).await?;
    let security = ConnectSecurity {
        tls: Some(pki.trust(None)?),
        bearer_token_file: None,
    };
    let client = TonicControl::connect(&served.https_endpoint(), TIMEOUT, security).await;
    // The certificate names `localhost`, the endpoint is an IP, so the handshake is refused.
    let failed = match client {
        Err(err) => matches!(err, ConnectError::Connect(_)),
        Ok(client) => client.get_actor(reference()).await.is_err(),
    };
    assert!(failed);
    Ok(())
}

#[test]
fn a_missing_ca_file_is_an_io_error() -> TestResult {
    let dir = TempDir::new()?;
    let err = TlsTrust::from_ca_file(dir.path().join("missing.pem"), None).err();
    assert_eq!(err.map(|err| err.kind()), Some(std::io::ErrorKind::NotFound));
    Ok(())
}

#[tokio::test]
async fn an_invalid_endpoint_is_reported_as_such() {
    let err = TonicControl::connect("", TIMEOUT, ConnectSecurity::default())
        .await
        .err();
    assert!(matches!(err, Some(ConnectError::Endpoint(_))), "{err:?}");
}

#[tokio::test]
async fn an_unreachable_server_is_a_connect_error() -> TestResult {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    drop(listener);
    let err = TonicControl::connect(
        &format!("http://{address}"),
        Duration::from_secs(2),
        ConnectSecurity::default(),
    )
    .await
    .err();
    assert!(matches!(err, Some(ConnectError::Connect(_))), "{err:?}");
    Ok(())
}

#[tokio::test]
async fn a_status_code_from_the_server_survives_the_auth_layer() -> TestResult {
    let dir = TempDir::new()?;
    let path = dir.path().join("token");
    std::fs::write(&path, "t")?;
    let served = Served::plaintext(FakeAte::failing(Code::PermissionDenied)).await?;
    let client = connect_with_token(&served, BearerTokenFile::new(&path)).await?;
    let err = client.get_actor(reference()).await.err();
    assert!(matches!(err, Some(AteError::PermissionDenied { .. })));
    Ok(())
}
