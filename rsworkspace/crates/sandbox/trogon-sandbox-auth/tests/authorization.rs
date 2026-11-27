use std::convert::Infallible;
use std::error::Error;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use authzed::v1::{CheckPermissionRequest, CheckPermissionResponse};
use tokio::net::TcpListener;
use tokio_stream::wrappers::TcpListenerStream;
use tonic::codegen::{Body, BoxFuture, Service, StdError, http};
use tonic::{Code, Status};
use trogon_sandbox_auth::{
    Authorizer, AuthzError, NoopAuthorizer, SandboxAuthzRequest, SpiceDbAuthorizer, SpiceDbClient, SpiceDbConnectError,
    SpiceDbEndpoint, SpiceDbEndpointError, SpiceDbToken, SpiceDbTokenError, TenantResourceType,
    TenantResourceTypeError,
};
use trogon_sandbox_contract::{ServiceAccountId, TenantId};

type TestResult = Result<(), Box<dyn Error>>;

const HAS_PERMISSION: i32 = 2;
const NO_PERMISSION: i32 = 1;

#[derive(Clone)]
enum Reply {
    Permissionship(i32),
    Failure(Code),
}

#[derive(Clone)]
struct Seen {
    request: CheckPermissionRequest,
    authorization: Option<String>,
}

#[derive(Clone)]
struct PermissionService {
    allowed_type: &'static str,
    reply: Option<Reply>,
    seen: Arc<Mutex<Vec<Seen>>>,
}

impl tonic::server::NamedService for PermissionService {
    const NAME: &'static str = "authzed.api.v1.PermissionsService";
}

struct CheckPermission {
    service: PermissionService,
    authorization: Option<String>,
}

impl tonic::server::UnaryService<CheckPermissionRequest> for CheckPermission {
    type Response = CheckPermissionResponse;
    type Future = BoxFuture<tonic::Response<Self::Response>, Status>;

    fn call(&mut self, request: tonic::Request<CheckPermissionRequest>) -> Self::Future {
        let request = request.into_inner();
        let reply = self.service.reply.clone().unwrap_or_else(|| {
            let allowed = request
                .resource
                .as_ref()
                .is_some_and(|resource| resource.object_type == self.service.allowed_type);
            Reply::Permissionship(if allowed { HAS_PERMISSION } else { NO_PERMISSION })
        });
        if let Ok(mut seen) = self.service.seen.lock() {
            seen.push(Seen {
                request,
                authorization: self.authorization.clone(),
            });
        }
        Box::pin(async move {
            match reply {
                Reply::Permissionship(permissionship) => Ok(tonic::Response::new(CheckPermissionResponse {
                    permissionship,
                    ..Default::default()
                })),
                Reply::Failure(code) => Err(Status::new(code, "fixture failure")),
            }
        })
    }
}

impl<B> Service<http::Request<B>> for PermissionService
where
    B: Body + Send + 'static,
    B::Error: Into<StdError> + Send + 'static,
{
    type Response = http::Response<tonic::body::Body>;
    type Error = Infallible;
    type Future = BoxFuture<Self::Response, Self::Error>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, request: http::Request<B>) -> Self::Future {
        let authorization = request
            .headers()
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let service = CheckPermission {
            service: self.clone(),
            authorization,
        };
        Box::pin(async move {
            let mut grpc = tonic::server::Grpc::new(tonic_prost::ProstCodec::default());
            Ok(grpc.unary(service, request).await)
        })
    }
}

struct Fixture {
    endpoint: SpiceDbEndpoint,
    seen: Arc<Mutex<Vec<Seen>>>,
    server: tokio::task::JoinHandle<()>,
}

impl Fixture {
    async fn start(allowed_type: &'static str, reply: Option<Reply>) -> Result<Self, Box<dyn Error>> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let endpoint = SpiceDbEndpoint::new(listener.local_addr()?.to_string())?;
        let seen = Arc::new(Mutex::new(Vec::new()));
        let service = PermissionService {
            allowed_type,
            reply,
            seen: seen.clone(),
        };
        let server = tokio::spawn(async move {
            let _ = tonic::transport::Server::builder()
                .add_service(service)
                .serve_with_incoming(TcpListenerStream::new(listener))
                .await;
        });
        Ok(Self { endpoint, seen, server })
    }

    fn seen(&self) -> Result<Vec<Seen>, Box<dyn Error>> {
        Ok(self.seen.lock().map_err(|_| "poisoned")?.clone())
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}

fn request() -> Result<SandboxAuthzRequest, Box<dyn Error>> {
    Ok(SandboxAuthzRequest {
        tenant: TenantId::new("tenant-a")?,
        principal: ServiceAccountId::new("service-a"),
    })
}

fn token() -> Result<SpiceDbToken, Box<dyn Error>> {
    Ok(SpiceDbToken::new("fixture-token")?)
}

#[tokio::test]
async fn noop_authorizer_always_allows() -> TestResult {
    NoopAuthorizer.authorize_sandbox_lease(&request()?).await?;
    Ok(())
}

#[tokio::test]
async fn default_authorizer_checks_the_sandbox_tenant_type_with_the_bearer_token() -> TestResult {
    let fixture = Fixture::start("sandbox_tenant", None).await?;
    let authorizer = SpiceDbAuthorizer::connect(&fixture.endpoint, &token()?).await?;
    authorizer.authorize_sandbox_lease(&request()?).await?;

    let seen = fixture.seen()?;
    assert_eq!(seen.len(), 1);
    let resource = seen[0].request.resource.as_ref().ok_or("resource")?;
    assert_eq!(resource.object_type, "sandbox_tenant");
    assert_eq!(resource.object_id, "tenant-a");
    assert_eq!(seen[0].request.permission, "lease_sandbox");
    let subject = seen[0]
        .request
        .subject
        .as_ref()
        .and_then(|s| s.object.as_ref())
        .ok_or("subject")?;
    assert_eq!(subject.object_type, "service_account");
    assert_eq!(subject.object_id, "service-a");
    assert_eq!(seen[0].authorization.as_deref(), Some("Bearer fixture-token"));
    Ok(())
}

#[tokio::test]
async fn a_grant_on_another_resource_type_does_not_authorize() -> TestResult {
    let fixture = Fixture::start("other_tenant", None).await?;
    let authorizer = SpiceDbAuthorizer::connect(&fixture.endpoint, &token()?).await?;
    let result = authorizer.authorize_sandbox_lease(&request()?).await;
    assert!(matches!(result, Err(AuthzError::Denied)));
    assert_eq!(fixture.seen()?.len(), 1, "a denial must not trigger another check");
    Ok(())
}

#[tokio::test]
async fn selected_resource_type_is_checked() -> TestResult {
    let fixture = Fixture::start("custom_tenant", None).await?;
    let authorizer =
        SpiceDbAuthorizer::connect_with_resource_type(&fixture.endpoint, &token()?, "custom_tenant".parse()?).await?;
    authorizer.authorize_sandbox_lease(&request()?).await?;
    let seen = fixture.seen()?;
    assert_eq!(
        seen[0].request.resource.as_ref().map(|r| r.object_type.as_str()),
        Some("custom_tenant")
    );
    Ok(())
}

#[tokio::test]
async fn permission_denied_and_not_found_statuses_deny() -> TestResult {
    for code in [Code::PermissionDenied, Code::NotFound] {
        let fixture = Fixture::start("", Some(Reply::Failure(code))).await?;
        let client = SpiceDbClient::connect(&fixture.endpoint, &token()?).await?;
        let result = client
            .check_sandbox_lease(&TenantId::new("t")?, &ServiceAccountId::new("s"))
            .await;
        assert!(matches!(result, Err(AuthzError::Denied)), "{code:?}");
    }
    Ok(())
}

#[tokio::test]
async fn other_failures_and_conditional_permission_are_not_allowed() -> TestResult {
    let fixture = Fixture::start("", Some(Reply::Failure(Code::Unavailable))).await?;
    let client = SpiceDbClient::connect(&fixture.endpoint, &token()?).await?;
    let error = client
        .check_sandbox_lease(&TenantId::new("t")?, &ServiceAccountId::new("s"))
        .await
        .err()
        .ok_or("an unavailable service must fail")?;
    assert!(matches!(error, AuthzError::Backend(_)));
    assert!(!format!("{error}").contains("fixture-token"));

    let fixture = Fixture::start("", Some(Reply::Permissionship(3))).await?;
    let client = SpiceDbClient::connect(&fixture.endpoint, &token()?).await?;
    let result = client
        .check_sandbox_lease(&TenantId::new("t")?, &ServiceAccountId::new("s"))
        .await;
    assert!(matches!(result, Err(AuthzError::Denied)));
    Ok(())
}

#[tokio::test]
async fn connecting_to_nothing_reports_a_transport_error_without_the_token() -> TestResult {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let endpoint = SpiceDbEndpoint::new(listener.local_addr()?.to_string())?;
    drop(listener);
    let error = SpiceDbClient::connect(&endpoint, &token()?)
        .await
        .err()
        .ok_or("nothing is listening")?;
    assert!(matches!(error, SpiceDbConnectError::Transport(_)));
    assert!(!format!("{error}").contains("fixture-token"));
    Ok(())
}

#[test]
fn authorizer_and_client_debug_output_omits_the_token() -> TestResult {
    let debug = format!("{:?}", token()?);
    assert_eq!(debug, "SpiceDbToken(***)");
    let error = SpiceDbConnectError::from(SpiceDbTokenError::Empty);
    assert!(matches!(error, SpiceDbConnectError::Token(SpiceDbTokenError::Empty)));
    Ok(())
}

#[tokio::test]
async fn client_debug_output_omits_the_token() -> TestResult {
    let fixture = Fixture::start("sandbox_tenant", None).await?;
    let client = SpiceDbClient::connect(&fixture.endpoint, &token()?).await?;
    let debug = format!("{client:?}");
    assert!(debug.contains("sandbox_tenant"));
    assert!(!debug.contains("fixture-token"));
    let debug = format!("{:?}", SpiceDbAuthorizer::new(client));
    assert!(!debug.contains("fixture-token"));
    Ok(())
}

#[test]
fn tokens_must_be_non_empty_and_header_safe() {
    assert!(matches!(SpiceDbToken::new(""), Err(SpiceDbTokenError::Empty)));
    assert!(matches!(SpiceDbToken::new("  "), Err(SpiceDbTokenError::Empty)));
    assert!(matches!(
        SpiceDbToken::new("line\nbreak"),
        Err(SpiceDbTokenError::NotHeaderSafe)
    ));
    assert!(!format!("{}", SpiceDbTokenError::NotHeaderSafe).is_empty());
}

#[test]
fn endpoints_default_to_http_and_must_be_uris() -> TestResult {
    assert_eq!(SpiceDbEndpoint::new("spicedb:50051")?.as_str(), "http://spicedb:50051");
    assert_eq!(
        SpiceDbEndpoint::new(" https://spicedb:443 ")?.to_string(),
        "https://spicedb:443"
    );
    assert_eq!(SpiceDbEndpoint::new("a:1")?, SpiceDbEndpoint::new("http://a:1")?);
    assert_eq!(SpiceDbEndpoint::new(" "), Err(SpiceDbEndpointError::Empty));
    assert!(matches!(
        SpiceDbEndpoint::new("bad host"),
        Err(SpiceDbEndpointError::InvalidUri(_))
    ));
    assert!(!format!("{}", SpiceDbEndpointError::Empty).is_empty());
    assert!(format!("{:?}", SpiceDbEndpoint::new("a:1")?).contains("http://a:1"));
    Ok(())
}

#[test]
fn resource_types_follow_schema_identifier_rules() -> TestResult {
    assert_eq!(TenantResourceType::default().as_str(), "sandbox_tenant");
    assert_eq!(TenantResourceType::default().to_string(), "sandbox_tenant");
    assert_eq!("a1_b".parse::<TenantResourceType>()?.as_str(), "a1_b");
    assert_eq!(TenantResourceType::new(""), Err(TenantResourceTypeError::Empty));
    assert_eq!(
        TenantResourceType::new("a".repeat(65)),
        Err(TenantResourceTypeError::TooLong { max: 64 })
    );
    for value in ["1tenant", "_tenant", "Tenant"] {
        assert_eq!(
            TenantResourceType::new(value),
            Err(TenantResourceTypeError::InvalidStart),
            "{value:?}"
        );
    }
    for value in ["sandbox tenant", "sandbox-tenant", "a/b", "tenantA"] {
        assert_eq!(
            TenantResourceType::new(value),
            Err(TenantResourceTypeError::InvalidCharacter),
            "{value:?}"
        );
    }
    assert!(!format!("{}", TenantResourceTypeError::Empty).is_empty());
    assert!(!format!("{}", TenantResourceTypeError::TooLong { max: 64 }).is_empty());
    assert!(!format!("{}", TenantResourceTypeError::InvalidStart).is_empty());
    assert!(!format!("{}", TenantResourceTypeError::InvalidCharacter).is_empty());
    Ok(())
}

#[test]
fn authz_request_carries_tenant_and_principal() -> TestResult {
    let request = request()?;
    assert_eq!(request.tenant.as_str(), "tenant-a");
    assert_eq!(request.principal.as_str(), "service-a");
    assert_eq!(request.clone(), request);
    assert_eq!(AuthzError::Denied.to_string(), "permission denied");
    Ok(())
}
