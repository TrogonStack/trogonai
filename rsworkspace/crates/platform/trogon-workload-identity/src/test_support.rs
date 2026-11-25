#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::collections::{BTreeMap, BTreeSet};
use std::convert::Infallible;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
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
    pub fn with_reported_id(mut self, id: &str) -> Self {
        self.id = id.to_owned();
        self
    }
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
    #[prost(btree_map = "string, bytes", tag = "3")]
    federated_bundles: BTreeMap<String, Vec<u8>>,
}

impl From<Vec<TestIdentity>> for WireResponse {
    fn from(identities: Vec<TestIdentity>) -> Self {
        Self {
            crls: Vec::new(),
            federated_bundles: BTreeMap::new(),
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

#[derive(Clone, PartialEq, prost::Message)]
struct WireJwtBundles {
    #[prost(btree_map = "string, bytes", tag = "1")]
    bundles: BTreeMap<String, Vec<u8>>,
}

type JwtUpdate = Option<Result<WireJwtBundles, Status>>;

/// What the fixture does once a client has opened the stream.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum StreamBehavior {
    /// Send the current response, then every replacement.
    Serve,
    /// Open the stream and never send a response.
    Silent,
    /// Open the stream and end it without a response.
    Closed,
}

#[derive(Clone)]
struct WorkloadService {
    updates: watch::Receiver<Result<WireResponse, Status>>,
    jwt_updates: watch::Receiver<JwtUpdate>,
    jwt_requests: Arc<AtomicUsize>,
    jwt_call_denied: Arc<AtomicBool>,
    requests: Arc<AtomicUsize>,
    header_names: Arc<Mutex<BTreeSet<String>>>,
    behavior: StreamBehavior,
}

type ResponseStream = Pin<Box<dyn Stream<Item = Result<WireResponse, Status>> + Send>>;
type BoxResponse<T> = Pin<Box<dyn Future<Output = Result<Response<T>, Status>> + Send>>;

impl tonic::server::ServerStreamingService<WireRequest> for WorkloadService {
    type Response = WireResponse;
    type ResponseStream = ResponseStream;
    type Future = BoxResponse<Self::ResponseStream>;

    fn call(&mut self, request: Request<WireRequest>) -> Self::Future {
        let updates = self.updates.clone();
        let behavior = self.behavior;
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
            match behavior {
                StreamBehavior::Serve => {}
                StreamBehavior::Silent => {
                    return Ok(Response::new(Box::pin(futures::stream::pending()) as ResponseStream));
                }
                StreamBehavior::Closed => {
                    return Ok(Response::new(Box::pin(futures::stream::empty()) as ResponseStream));
                }
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

/// Serves `FetchJWTBundles`: nothing until the first set is published, then
/// the current set and every replacement.
#[derive(Clone)]
struct JwtBundlesService {
    updates: watch::Receiver<JwtUpdate>,
    requests: Arc<AtomicUsize>,
    denied: Arc<AtomicBool>,
}

type JwtBundlesStream = Pin<Box<dyn Stream<Item = Result<WireJwtBundles, Status>> + Send>>;

impl tonic::server::ServerStreamingService<WireRequest> for JwtBundlesService {
    type Response = WireJwtBundles;
    type ResponseStream = JwtBundlesStream;
    type Future = BoxResponse<Self::ResponseStream>;

    fn call(&mut self, request: Request<WireRequest>) -> Self::Future {
        let updates = self.updates.clone();
        self.requests.fetch_add(1, Ordering::SeqCst);
        let denied = self.denied.load(Ordering::SeqCst);
        Box::pin(async move {
            if request
                .metadata()
                .get("workload.spiffe.io")
                .and_then(|value| value.to_str().ok())
                != Some("true")
            {
                return Err(Status::invalid_argument("required workload header"));
            }
            if denied {
                return Err(Status::permission_denied("bundles withdrawn"));
            }
            let stream = futures::stream::unfold((updates, true), |(mut updates, mut first)| async move {
                loop {
                    if !first && updates.changed().await.is_err() {
                        return None;
                    }
                    first = false;
                    let value = updates.borrow_and_update().clone();
                    if let Some(value) = value {
                        return Some((value, (updates, false)));
                    }
                }
            });
            Ok(Response::new(Box::pin(stream) as JwtBundlesStream))
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
        service
            .header_names
            .lock()
            .unwrap()
            .extend(request.headers().keys().map(|name| name.as_str().to_owned()));
        Box::pin(async move {
            match request.uri().path() {
                "/SpiffeWorkloadAPI/FetchX509SVID" => {
                    let mut grpc = tonic::server::Grpc::new(tonic_prost::ProstCodec::default());
                    Ok(grpc.server_streaming(service, request).await)
                }
                "/SpiffeWorkloadAPI/FetchJWTBundles" => {
                    let service = JwtBundlesService {
                        updates: service.jwt_updates,
                        requests: service.jwt_requests,
                        denied: service.jwt_call_denied,
                    };
                    let mut grpc = tonic::server::Grpc::new(tonic_prost::ProstCodec::default());
                    Ok(grpc.server_streaming(service, request).await)
                }
                _ => Ok(Status::unimplemented("RPC").into_http()),
            }
        })
    }
}

impl tonic::server::NamedService for WorkloadService {
    const NAME: &'static str = "SpiffeWorkloadAPI";
}

pub struct WorkloadApiFixture {
    endpoint: UnixEndpoint,
    updates: watch::Sender<Result<WireResponse, Status>>,
    jwt_updates: watch::Sender<JwtUpdate>,
    requests: Arc<AtomicUsize>,
    jwt_requests: Arc<AtomicUsize>,
    jwt_call_denied: Arc<AtomicBool>,
    header_names: Arc<Mutex<BTreeSet<String>>>,
    shutdown: CancellationToken,
    task: JoinHandle<()>,
    directory: tempfile::TempDir,
}

impl WorkloadApiFixture {
    pub async fn start(identities: Vec<TestIdentity>) -> Self {
        Self::start_with(identities, StreamBehavior::Serve).await
    }
    pub async fn start_with(identities: Vec<TestIdentity>, behavior: StreamBehavior) -> Self {
        let directory = tempfile::tempdir().expect("socket directory");
        let path = directory.path().join("workload.sock");
        let listener = UnixListener::bind(&path).expect("Unix listener");
        let endpoint = UnixEndpoint::new(&format!("unix://{}", path.display())).expect("Unix endpoint");
        let (updates, receiver) = watch::channel(Ok(identities.into()));
        let requests = Arc::new(AtomicUsize::new(0));
        let (jwt_updates, jwt_receiver) = watch::channel(None);
        let jwt_requests = Arc::new(AtomicUsize::new(0));
        let jwt_call_denied = Arc::new(AtomicBool::new(false));
        let header_names = Arc::new(Mutex::new(BTreeSet::new()));
        let service = WorkloadService {
            header_names: Arc::clone(&header_names),
            updates: receiver,
            jwt_updates: jwt_receiver,
            jwt_requests: Arc::clone(&jwt_requests),
            jwt_call_denied: Arc::clone(&jwt_call_denied),
            requests: Arc::clone(&requests),
            behavior,
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
            jwt_updates,
            requests,
            jwt_requests,
            jwt_call_denied,
            header_names,
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
    pub fn replace_with_federated_bundles(&self, identities: Vec<TestIdentity>, bundles: BTreeMap<String, Vec<u8>>) {
        let mut response = WireResponse::from(identities);
        response.federated_bundles = bundles;
        drop(self.updates.send_replace(Ok(response)));
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
    /// Publishes the JWT bundle set, as `(trust domain, JWKS document)` pairs.
    pub fn replace_jwt_bundles(&self, bundles: &[(&str, &str)]) {
        let bundles = bundles
            .iter()
            .map(|(domain, jwks)| ((*domain).to_owned(), jwks.as_bytes().to_vec()))
            .collect();
        drop(self.jwt_updates.send_replace(Some(Ok(WireJwtBundles { bundles }))));
    }
    pub fn jwt_bundles_permission_denied(&self) {
        drop(
            self.jwt_updates
                .send_replace(Some(Err(Status::permission_denied("bundles withdrawn")))),
        );
    }
    /// Makes every later `FetchJWTBundles` call fail with `PermissionDenied`
    /// before any stream opens.
    pub fn deny_jwt_bundle_calls(&self) {
        self.jwt_call_denied.store(true, Ordering::SeqCst);
    }
    pub fn jwt_bundles_unavailable(&self) {
        drop(
            self.jwt_updates
                .send_replace(Some(Err(Status::unavailable("agent restarting")))),
        );
    }
    pub fn jwt_bundle_requests(&self) -> usize {
        self.jwt_requests.load(Ordering::SeqCst)
    }
    /// Every HTTP/2 header name any request has carried so far.
    pub fn request_header_names(&self) -> BTreeSet<String> {
        self.header_names.lock().unwrap().clone()
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
