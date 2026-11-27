use std::convert::Infallible;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::task::{Context, Poll};
use std::time::Duration;

use prost::Message;
use tonic::codegen::{Body, BoxFuture, Service, StdError, http};
use trogon_sandbox_substrate_worker::ResourceRevision;
use trogon_sandbox_substrate_worker::proto;
use trogon_sandbox_substrate_worker::{ConnectSecurity, Control, TonicControl};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

// Field numbers and enum values are independently pinned to the v1.0.0 wire contract.
#[derive(Clone, Message)]
struct ResourceMetadata {
    #[prost(string, tag = "1")]
    atespace: String,
    #[prost(string, tag = "2")]
    name: String,
    #[prost(string, tag = "3")]
    uid: String,
    #[prost(int64, tag = "4")]
    version: i64,
}

#[derive(Clone, Message)]
struct ActorStatus {
    #[prost(int32, tag = "1")]
    state: i32,
}

#[derive(Clone, Message)]
struct Actor {
    #[prost(message, optional, tag = "1")]
    metadata: Option<ResourceMetadata>,
    #[prost(message, optional, tag = "5")]
    status: Option<ActorStatus>,
}

#[derive(Clone, Message)]
struct ObjectRef {
    #[prost(string, tag = "1")]
    atespace: String,
    #[prost(string, tag = "2")]
    name: String,
}

#[derive(Clone, Message)]
struct Request {
    #[prost(message, optional, tag = "1")]
    actor: Option<ObjectRef>,
    #[prost(message, optional, tag = "2")]
    options: Option<SuspendOptions>,
}

#[derive(Clone, Message)]
struct SuspendOptions {
    #[prost(int32, tag = "1")]
    mode: i32,
}

#[derive(Clone, Message)]
struct DeleteRequest {
    #[prost(message, optional, tag = "1")]
    resource: Option<ObjectRef>,
    #[prost(message, optional, tag = "2")]
    options: Option<DeleteOptions>,
}

#[derive(Clone, Message)]
struct DeleteOptions {
    #[prost(int64, tag = "1")]
    version: i64,
    #[prost(string, tag = "2")]
    uid: String,
}

#[derive(Clone, Message)]
struct LifecycleResponse {
    #[prost(message, optional, tag = "1")]
    actor: Option<Actor>,
    #[prost(bool, tag = "2")]
    resumed: bool,
}

fn actor(state: i32) -> Actor {
    Actor {
        metadata: Some(ResourceMetadata {
            atespace: "tenant".into(),
            name: "actor".into(),
            uid: "3f234d36-857d-474f-8009-64fdfe4aa84f".into(),
            version: 17,
        }),
        status: Some(ActorStatus { state }),
    }
}

struct Delete {
    method: String,
    requests: Arc<Mutex<Vec<(String, DeleteRequest)>>>,
}
impl tonic::server::UnaryService<DeleteRequest> for Delete {
    type Response = Actor;
    type Future = BoxFuture<tonic::Response<Actor>, tonic::Status>;
    fn call(&mut self, request: tonic::Request<DeleteRequest>) -> Self::Future {
        locked(&self.requests).push((self.method.clone(), request.into_inner()));
        let mut response = actor(6);
        response.status = None;
        Box::pin(async move { Ok(tonic::Response::new(response)) })
    }
}

struct GetActor;
impl tonic::server::UnaryService<Request> for GetActor {
    type Response = Actor;
    type Future = BoxFuture<tonic::Response<Actor>, tonic::Status>;
    fn call(&mut self, _: tonic::Request<Request>) -> Self::Future {
        Box::pin(async { Ok(tonic::Response::new(actor(2))) })
    }
}

struct Lifecycle {
    method: String,
    requests: Arc<Mutex<Vec<Request>>>,
}
impl tonic::server::UnaryService<Request> for Lifecycle {
    type Response = LifecycleResponse;
    type Future = BoxFuture<tonic::Response<LifecycleResponse>, tonic::Status>;
    fn call(&mut self, request: tonic::Request<Request>) -> Self::Future {
        let state = if self.method == "SuspendActor" { 4 } else { 2 };
        let mut response_actor = actor(state);
        if self.method == "SuspendActor" {
            locked(&self.requests).push(request.into_inner());
            response_actor.status = None;
        }
        Box::pin(async move {
            Ok(tonic::Response::new(LifecycleResponse {
                actor: Some(response_actor),
                resumed: false,
            }))
        })
    }
}

#[derive(Clone, Default)]
struct V1Control {
    suspend_requests: Arc<Mutex<Vec<Request>>>,
    delete_requests: Arc<Mutex<Vec<(String, DeleteRequest)>>>,
    actor_reads: Arc<AtomicUsize>,
}

impl tonic::server::NamedService for V1Control {
    const NAME: &'static str = "substrate.ateapi.v1.Control";
}

impl<B> Service<http::Request<B>> for V1Control
where
    B: Body + Send + 'static,
    B::Error: Into<StdError> + Send + 'static,
{
    type Response = http::Response<tonic::body::Body>;
    type Error = Infallible;
    type Future = BoxFuture<Self::Response, Self::Error>;

    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, request: http::Request<B>) -> Self::Future {
        let method = request.uri().path().rsplit('/').next().unwrap_or_default().to_owned();
        if method == "DeleteActor" || method == "DeleteActorTemplate" {
            let service = Delete {
                method,
                requests: self.delete_requests.clone(),
            };
            return Box::pin(async move {
                let mut grpc = tonic::server::Grpc::new(tonic_prost::ProstCodec::default());
                Ok(grpc.unary(service, request).await)
            });
        }
        if method == "GetActor" {
            self.actor_reads.fetch_add(1, Ordering::SeqCst);
            return Box::pin(async move {
                let mut grpc = tonic::server::Grpc::new(tonic_prost::ProstCodec::default());
                Ok(grpc.unary(GetActor, request).await)
            });
        }
        let service = Lifecycle {
            method,
            requests: self.suspend_requests.clone(),
        };
        Box::pin(async move {
            let mut grpc = tonic::server::Grpc::new(tonic_prost::ProstCodec::default());
            Ok(grpc.unary(service, request).await)
        })
    }
}

#[derive(Clone)]
struct BetaRouteAlias(V1Control);

impl tonic::server::NamedService for BetaRouteAlias {
    const NAME: &'static str = "ateapi.Control";
}

impl<B> Service<http::Request<B>> for BetaRouteAlias
where
    B: Body + Send + 'static,
    B::Error: Into<StdError> + Send + 'static,
{
    type Response = http::Response<tonic::body::Body>;
    type Error = Infallible;
    type Future = BoxFuture<Self::Response, Self::Error>;
    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }
    fn call(&mut self, request: http::Request<B>) -> Self::Future {
        self.0.call(request)
    }
}

struct Incoming(tokio::net::TcpListener);

impl futures::Stream for Incoming {
    type Item = std::io::Result<tokio::net::TcpStream>;
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.0
            .poll_accept(cx)
            .map(|value| Some(value.map(|(stream, _)| stream)))
    }
}

struct ServerTask(tokio::task::JoinHandle<()>);

impl Drop for ServerTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn connect(allow_beta_route: bool) -> Result<(TonicControl, V1Control, ServerTask), Box<dyn std::error::Error>> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let endpoint = format!("http://{}", listener.local_addr()?);
    let service = V1Control::default();
    let fixture = service.clone();
    let server = ServerTask(tokio::spawn(async move {
        let _ = tonic::transport::Server::builder()
            .add_service(service.clone())
            .add_optional_service(allow_beta_route.then_some(BetaRouteAlias(service)))
            .serve_with_incoming(Incoming(listener))
            .await;
    }));
    let client = TonicControl::connect(&endpoint, Duration::from_secs(3), ConnectSecurity::default()).await?;
    Ok((client, fixture, server))
}

fn reference() -> proto::ObjectRef {
    proto::ObjectRef {
        atespace: "tenant".into(),
        name: "actor".into(),
    }
}

#[tokio::test]
async fn tonic_control_uses_the_official_v1_service_route() -> TestResult {
    let (client, _, _server) = connect(false).await?;
    let actual = client.get_actor(reference()).await?;
    assert_eq!(actual.metadata.ok_or("metadata")?.version, 17);
    Ok(())
}

#[test]
fn official_actor_states_decode_without_beta_aliasing() -> TestResult {
    for (wire, expected) in [
        (5, "ACTOR_STATE_CRASHED"),
        (6, "ACTOR_STATE_DELETING"),
        (7, "ACTOR_STATE_REVERTING"),
    ] {
        let decoded = proto::ActorStatus::decode(ActorStatus { state: wire }.encode_to_vec().as_slice())?;
        assert_eq!(decoded.state().as_str_name(), expected, "official state {wire}");
    }
    Ok(())
}

#[tokio::test]
async fn already_running_resume_is_successful_on_the_official_v1_route() -> TestResult {
    let (client, _, _server) = connect(false).await?;
    let actual = client.resume_actor(reference()).await?;
    assert_eq!(
        actual.status.ok_or("status")?.state().as_str_name(),
        "ACTOR_STATE_RUNNING"
    );
    Ok(())
}

#[tokio::test]
async fn retained_actor_suspension_explicitly_requests_synchronous_durability() -> TestResult {
    let (client, fixture, _server) = connect(true).await?;
    client.suspend_actor(reference()).await?;
    let requests = locked(&fixture.suspend_requests);
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].options.as_ref().map(|options| options.mode), Some(1));
    Ok(())
}

#[tokio::test]
async fn native_deletions_preserve_the_callers_uid_and_observed_version() -> TestResult {
    let (client, fixture, _server) = connect(false).await?;
    let metadata = proto::ResourceMetadata {
        atespace: "tenant".into(),
        name: "held-resource".into(),
        uid: "8c7a5db0-dc71-4b6f-a1d1-34ad19fc2230".into(),
        version: 11,
        ..Default::default()
    };
    let held = ResourceRevision::from_metadata(Some(&metadata))?;
    client.delete_actor(held.clone()).await?;
    client.delete_actor_template(held).await?;
    let requests = locked(&fixture.delete_requests);
    assert_eq!(requests.len(), 2);
    for ((method, request), expected_method) in requests.iter().zip(["DeleteActor", "DeleteActorTemplate"]) {
        assert_eq!(method, expected_method);
        let resource = request.resource.as_ref().ok_or("resource")?;
        assert_eq!(resource.atespace, metadata.atespace);
        assert_eq!(resource.name, metadata.name);
        let options = request.options.as_ref().ok_or("native deletion guards")?;
        assert_eq!(options.uid, metadata.uid);
        assert_eq!(options.version, metadata.version);
    }
    assert_eq!(
        fixture.actor_reads.load(Ordering::SeqCst),
        0,
        "deletion must not obtain authority by reading a replacement at the held name"
    );
    Ok(())
}

#[test]
fn incomplete_observed_identity_cannot_authorize_native_deletion() {
    assert!(ResourceRevision::from_metadata(None).is_err());
    let complete = proto::ResourceMetadata {
        atespace: "tenant".into(),
        name: "actor".into(),
        uid: "3f234d36-857d-474f-8009-64fdfe4aa84f".into(),
        version: 17,
        ..Default::default()
    };
    let mut cleared = Vec::new();
    for field in 0..4 {
        let mut incomplete = complete.clone();
        match field {
            0 => incomplete.atespace.clear(),
            1 => incomplete.name.clear(),
            2 => incomplete.uid.clear(),
            _ => incomplete.version = 0,
        }
        cleared.push(incomplete);
    }
    for incomplete in &cleared {
        assert!(ResourceRevision::from_metadata(Some(incomplete)).is_err());
    }
}
