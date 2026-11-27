//! A fake `substrate.ateapi.v1.Control` server for the client tests. Clients only are
//! generated from the proto, so the server side is hand-rolled over `tonic::server::Grpc`.

use std::convert::Infallible;
use std::error::Error;
use std::fmt::Debug;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::task::{Context, Poll};
use std::time::Duration;

use prost::Message;
use tonic::Code;
use tonic::codegen::{Body, BoxFuture, Service, StdError, http};
use tonic::transport::{Identity, ServerTlsConfig};

use crate::ate_client::{ConnectSecurity, TonicControl};
use crate::proto;

/// One request the fake received.
#[derive(Clone, Debug)]
pub(super) struct Call {
    pub(super) method: String,
    pub(super) authorization: Option<String>,
    pub(super) request: String,
}

#[derive(Clone, Default)]
pub(super) struct FakeAte {
    calls: Arc<Mutex<Vec<Call>>>,
    fail_with: Option<Code>,
    omit_actor: bool,
}

pub(super) fn actor() -> proto::Actor {
    proto::Actor {
        metadata: Some(metadata("actor")),
        ..Default::default()
    }
}

fn metadata(name: &str) -> proto::ResourceMetadata {
    proto::ResourceMetadata {
        atespace: "tenant".into(),
        name: name.into(),
        uid: "3f234d36-857d-474f-8009-64fdfe4aa84f".into(),
        version: 17,
        ..Default::default()
    }
}

fn page(request: &proto::ListActorsRequest) -> proto::ListActorsResponse {
    let token = request.options.as_ref().map(|options| options.page_token.as_str());
    let (name, next) = match token {
        Some("page-2") => ("second", ""),
        _ => ("first", "page-2"),
    };
    proto::ListActorsResponse {
        actors: vec![proto::Actor {
            metadata: Some(metadata(name)),
            ..Default::default()
        }],
        next_page_token: next.into(),
    }
}

impl FakeAte {
    pub(super) fn failing(code: Code) -> Self {
        Self {
            fail_with: Some(code),
            ..Self::default()
        }
    }

    pub(super) fn without_actor() -> Self {
        Self {
            omit_actor: true,
            ..Self::default()
        }
    }

    pub(super) fn calls(&self) -> Vec<Call> {
        self.locked().clone()
    }

    fn locked(&self) -> MutexGuard<'_, Vec<Call>> {
        self.calls.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn begin<Req: Debug>(&self, method: &str, request: &tonic::Request<Req>) -> Result<(), tonic::Status> {
        let authorization = request
            .metadata()
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        self.locked().push(Call {
            method: method.to_string(),
            authorization,
            request: format!("{:?}", request.get_ref()),
        });
        match self.fail_with {
            Some(code) => Err(tonic::Status::new(code, "scripted failure")),
            None => Ok(()),
        }
    }

    fn lifecycle_actor(&self) -> Option<proto::Actor> {
        (!self.omit_actor).then(actor)
    }

    async fn unary<B, Req, Resp>(
        &self,
        method: &str,
        request: http::Request<B>,
        respond: fn(&Req) -> Resp,
    ) -> http::Response<tonic::body::Body>
    where
        B: Body + Send + 'static,
        B::Error: Into<StdError> + Send + 'static,
        Req: Message + Default + Debug + Send + Sync + 'static,
        Resp: Message + Send + Sync + 'static,
    {
        let service = Handler {
            fake: self.clone(),
            method: method.to_string(),
            respond,
        };
        tonic::server::Grpc::new(tonic_prost::ProstCodec::default())
            .unary(service, request)
            .await
    }
}

struct Handler<Req, Resp> {
    fake: FakeAte,
    method: String,
    respond: fn(&Req) -> Resp,
}

impl<Req, Resp> tonic::server::UnaryService<Req> for Handler<Req, Resp>
where
    Req: Debug + Send + Sync + 'static,
    Resp: Send + Sync + 'static,
{
    type Response = Resp;
    type Future = BoxFuture<tonic::Response<Resp>, tonic::Status>;

    fn call(&mut self, request: tonic::Request<Req>) -> Self::Future {
        let outcome = self
            .fake
            .begin(&self.method, &request)
            .map(|()| tonic::Response::new((self.respond)(request.get_ref())));
        Box::pin(async move { outcome })
    }
}

impl tonic::server::NamedService for FakeAte {
    const NAME: &'static str = "substrate.ateapi.v1.Control";
}

impl<B> Service<http::Request<B>> for FakeAte
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
        let fake = self.clone();
        let method = request.uri().path().rsplit('/').next().unwrap_or_default().to_string();
        Box::pin(async move {
            let response = match method.as_str() {
                "CreateAtespace" => {
                    fake.unary(&method, request, |_: &proto::CreateAtespaceRequest| proto::Atespace {
                        metadata: Some(metadata("tenant")),
                    })
                    .await
                }
                "GetActor" => fake.unary(&method, request, |_: &proto::GetActorRequest| actor()).await,
                "CreateActor" => {
                    fake.unary(&method, request, |_: &proto::CreateActorRequest| actor())
                        .await
                }
                "DeleteActor" => {
                    fake.unary(&method, request, |_: &proto::DeleteActorRequest| actor())
                        .await
                }
                "ListActors" => fake.unary(&method, request, page).await,
                "CreateActorTemplate" => {
                    fake.unary(&method, request, |_: &proto::CreateActorTemplateRequest| {
                        proto::ActorTemplate::default()
                    })
                    .await
                }
                "GetActorTemplate" => {
                    fake.unary(&method, request, |_: &proto::GetActorTemplateRequest| {
                        proto::ActorTemplate::default()
                    })
                    .await
                }
                "DeleteActorTemplate" => {
                    fake.unary(&method, request, |_: &proto::DeleteActorTemplateRequest| {
                        proto::ActorTemplate::default()
                    })
                    .await
                }
                "ResumeActor" => {
                    let lifecycle = fake.lifecycle_actor();
                    let respond = if lifecycle.is_some() {
                        |_: &proto::ResumeActorRequest| proto::ResumeActorResponse {
                            actor: Some(actor()),
                            resumed: false,
                        }
                    } else {
                        |_: &proto::ResumeActorRequest| proto::ResumeActorResponse::default()
                    };
                    fake.unary(&method, request, respond).await
                }
                "SuspendActor" => {
                    let respond = if fake.lifecycle_actor().is_some() {
                        |_: &proto::SuspendActorRequest| proto::SuspendActorResponse { actor: Some(actor()) }
                    } else {
                        |_: &proto::SuspendActorRequest| proto::SuspendActorResponse::default()
                    };
                    fake.unary(&method, request, respond).await
                }
                _ => tonic::Status::unimplemented(method).into_http(),
            };
            Ok(response)
        })
    }
}

struct Incoming(tokio::net::TcpListener);

impl futures::Stream for Incoming {
    type Item = std::io::Result<tokio::net::TcpStream>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.0
            .poll_accept(cx)
            .map(|accepted| Some(accepted.map(|(stream, _)| stream)))
    }
}

/// A running fake. Dropping it stops the server.
pub(super) struct Served {
    pub(super) fake: FakeAte,
    pub(super) address: std::net::SocketAddr,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Served {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Served {
    pub(super) async fn plaintext(fake: FakeAte) -> Result<Self, Box<dyn Error>> {
        Self::start(fake, None).await
    }

    pub(super) async fn tls(fake: FakeAte, certificate_pem: &str, key_pem: &str) -> Result<Self, Box<dyn Error>> {
        Self::start(fake, Some(Identity::from_pem(certificate_pem, key_pem))).await
    }

    async fn start(fake: FakeAte, identity: Option<Identity>) -> Result<Self, Box<dyn Error>> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let service = fake.clone();
        let mut builder = tonic::transport::Server::builder();
        if let Some(identity) = identity {
            builder = builder.tls_config(ServerTlsConfig::new().identity(identity))?;
        }
        let router = builder.add_service(service);
        let task = tokio::spawn(async move {
            // The task is aborted on drop; a serve error only ever surfaces as a client failure.
            let _ = router.serve_with_incoming(Incoming(listener)).await;
        });
        Ok(Self { fake, address, task })
    }

    pub(super) fn http_endpoint(&self) -> String {
        format!("http://{}", self.address)
    }

    pub(super) fn https_endpoint(&self) -> String {
        format!("https://{}", self.address)
    }

    pub(super) async fn client(&self) -> Result<TonicControl, Box<dyn Error>> {
        Ok(TonicControl::connect(
            &self.http_endpoint(),
            Duration::from_secs(5),
            ConnectSecurity::default(),
        )
        .await?)
    }
}
