#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::convert::Infallible;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll};
use std::time::Duration;

use futures::Stream;
use rcgen::{
    BasicConstraints, CertificateParams, CertifiedIssuer, DistinguishedName, ExtendedKeyUsagePurpose, IsCa, KeyPair,
    KeyUsagePurpose, SanType, string::Ia5String,
};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use sha2::{Digest, Sha256};
use tokio::net::UnixListener;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio_stream::wrappers::UnixListenerStream;
use tokio_util::sync::CancellationToken;
use tonic::body::Body;
use tonic::codegen::{Service, http};
use tonic::{Request, Response, Status};

use crate::UnixEndpoint;

pub struct TestCa(CertifiedIssuer<'static, KeyPair>);

#[derive(Clone)]
pub struct TestIdentity {
    id: String,
    chain: Vec<u8>,
    key: Vec<u8>,
    bundle: Vec<u8>,
}

impl TestCa {
    pub fn with_lifetime(lifetime: Duration) -> Self {
        let mut params = CertificateParams::default();
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
        params.not_before = time::OffsetDateTime::now_utc() - time::Duration::seconds(1);
        params.not_after = time::OffsetDateTime::now_utc() + time::Duration::try_from(lifetime).expect("CA lifetime");
        Self(CertifiedIssuer::self_signed(params, KeyPair::generate().expect("CA key")).expect("CA"))
    }
    pub fn new() -> Self {
        let mut params = CertificateParams::default();
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
        Self(CertifiedIssuer::self_signed(params, KeyPair::generate().expect("CA key")).expect("CA"))
    }

    pub fn issue(&self, id: &str, lifetime: Duration) -> TestIdentity {
        let mut params = CertificateParams::default();
        params.distinguished_name = DistinguishedName::new();
        params.is_ca = IsCa::ExplicitNoCa;
        params
            .subject_alt_names
            .push(SanType::URI(Ia5String::try_from(id).expect("URI")));
        params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth, ExtendedKeyUsagePurpose::ClientAuth];
        let now = time::OffsetDateTime::now_utc();
        params.not_before = now - time::Duration::seconds(1);
        params.not_after = now + time::Duration::try_from(lifetime).expect("lifetime");
        self.issue_with_params(id, params)
    }

    pub fn issue_with_params(&self, id: &str, params: CertificateParams) -> TestIdentity {
        let key = KeyPair::generate().expect("workload key");
        let certificate = params.signed_by(&key, &self.0).expect("workload certificate");
        TestIdentity {
            id: id.to_owned(),
            chain: certificate.der().to_vec(),
            key: key.serialize_der(),
            bundle: self.0.der().to_vec(),
        }
    }

    pub fn root(&self) -> CertificateDer<'static> {
        self.0.der().clone()
    }
    pub fn root_sha256(&self) -> [u8; 32] {
        Sha256::digest(self.0.der()).into()
    }
}

impl Default for TestCa {
    fn default() -> Self {
        Self::new()
    }
}

impl TestIdentity {
    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn certificates(&self) -> Vec<CertificateDer<'static>> {
        vec![CertificateDer::from(self.chain.clone())]
    }
    pub fn private_key(&self) -> PrivateKeyDer<'static> {
        PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(self.key.clone()))
    }
    pub fn with_private_key(mut self, other: &TestIdentity) -> Self {
        self.key.clone_from(&other.key);
        self
    }
    pub fn with_bundle(mut self, roots: &[CertificateDer<'_>]) -> Self {
        self.bundle = roots.iter().flat_map(|root| root.as_ref().iter().copied()).collect();
        self
    }
}

#[derive(Clone, PartialEq, prost::Message)]
struct WireRequest {}

#[derive(Clone, PartialEq, prost::Message)]
struct WireIdentity {
    #[prost(string, tag = "1")]
    spiffe_id: String,
    #[prost(bytes = "vec", tag = "2")]
    x509_svid: Vec<u8>,
    #[prost(bytes = "vec", tag = "3")]
    x509_svid_key: Vec<u8>,
    #[prost(bytes = "vec", tag = "4")]
    bundle: Vec<u8>,
    #[prost(string, tag = "5")]
    hint: String,
}

#[derive(Clone, PartialEq, prost::Message)]
struct WireResponse {
    #[prost(message, repeated, tag = "1")]
    svids: Vec<WireIdentity>,
    #[prost(bytes = "vec", repeated, tag = "2")]
    crls: Vec<Vec<u8>>,
}

impl From<Vec<TestIdentity>> for WireResponse {
    fn from(identities: Vec<TestIdentity>) -> Self {
        Self {
            crls: Vec::new(),
            svids: identities
                .into_iter()
                .map(|id| WireIdentity {
                    spiffe_id: id.id,
                    x509_svid: id.chain,
                    x509_svid_key: id.key,
                    bundle: id.bundle,
                    hint: String::new(),
                })
                .collect(),
        }
    }
}

#[derive(Clone)]
struct WorkloadService {
    updates: watch::Receiver<Result<WireResponse, Status>>,
    requests: Arc<AtomicUsize>,
}

type ResponseStream = Pin<Box<dyn Stream<Item = Result<WireResponse, Status>> + Send>>;
type BoxResponse<T> = Pin<Box<dyn Future<Output = Result<Response<T>, Status>> + Send>>;

impl tonic::server::ServerStreamingService<WireRequest> for WorkloadService {
    type Response = WireResponse;
    type ResponseStream = ResponseStream;
    type Future = BoxResponse<Self::ResponseStream>;

    fn call(&mut self, request: Request<WireRequest>) -> Self::Future {
        let updates = self.updates.clone();
        self.requests.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            if request
                .metadata()
                .get("workload.spiffe.io")
                .and_then(|value| value.to_str().ok())
                != Some("true")
            {
                return Err(Status::invalid_argument("required workload header"));
            }
            let stream = futures::stream::unfold((updates, true), |(mut updates, first)| async move {
                if !first && updates.changed().await.is_err() {
                    return None;
                }
                let value = updates.borrow_and_update().clone();
                Some((value, (updates, false)))
            });
            Ok(Response::new(Box::pin(stream) as ResponseStream))
        })
    }
}

impl<B> Service<http::Request<B>> for WorkloadService
where
    B: tonic::codegen::Body + Send + 'static,
    B::Error: Into<tonic::codegen::StdError> + Send + 'static,
{
    type Response = http::Response<Body>;
    type Error = Infallible;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }
    fn call(&mut self, request: http::Request<B>) -> Self::Future {
        let service = self.clone();
        Box::pin(async move {
            if request.uri().path() != "/SpiffeWorkloadAPI/FetchX509SVID" {
                return Ok(Status::unimplemented("RPC").into_http());
            }
            let codec = tonic_prost::ProstCodec::default();
            let mut grpc = tonic::server::Grpc::new(codec);
            Ok(grpc.server_streaming(service, request).await)
        })
    }
}

impl tonic::server::NamedService for WorkloadService {
    const NAME: &'static str = "SpiffeWorkloadAPI";
}

pub struct WorkloadApiFixture {
    endpoint: UnixEndpoint,
    updates: watch::Sender<Result<WireResponse, Status>>,
    requests: Arc<AtomicUsize>,
    shutdown: CancellationToken,
    task: JoinHandle<()>,
    directory: tempfile::TempDir,
}

impl WorkloadApiFixture {
    pub async fn start(identities: Vec<TestIdentity>) -> Self {
        let directory = tempfile::tempdir().expect("socket directory");
        let path = directory.path().join("workload.sock");
        let listener = UnixListener::bind(&path).expect("Unix listener");
        let endpoint = UnixEndpoint::new(&format!("unix://{}", path.display())).expect("Unix endpoint");
        let (updates, receiver) = watch::channel(Ok(identities.into()));
        let requests = Arc::new(AtomicUsize::new(0));
        let service = WorkloadService {
            updates: receiver,
            requests: Arc::clone(&requests),
        };
        let shutdown = CancellationToken::new();
        let stop = shutdown.clone();
        let task = tokio::spawn(async move {
            tonic::transport::Server::builder()
                .add_service(service)
                .serve_with_incoming_shutdown(UnixListenerStream::new(listener), stop.cancelled())
                .await
                .expect("Workload API server");
        });
        Self {
            endpoint,
            updates,
            requests,
            shutdown,
            task,
            directory,
        }
    }
    pub fn endpoint(&self) -> UnixEndpoint {
        self.endpoint.clone()
    }
    pub fn replace(&self, identities: Vec<TestIdentity>) {
        drop(self.updates.send_replace(Ok(identities.into())));
    }
    pub fn replace_with_crls(&self, identities: Vec<TestIdentity>, crls: Vec<Vec<u8>>) {
        let mut response = WireResponse::from(identities);
        response.crls = crls;
        drop(self.updates.send_replace(Ok(response)));
    }
    pub fn permission_denied(&self) {
        drop(
            self.updates
                .send_replace(Err(Status::permission_denied("identity withdrawn"))),
        );
    }
    pub fn requests(&self) -> usize {
        self.requests.load(Ordering::SeqCst)
    }
    pub fn socket_exists(&self) -> bool {
        self.directory.path().join("workload.sock").exists()
    }
    pub async fn stop(self) {
        self.shutdown.cancel();
        self.task.abort();
        let _ = self.task.await;
    }
}

#[cfg(test)]
mod tests;
