use async_trait::async_trait;
use authzed::v1::permissions_service_client::PermissionsServiceClient;
use authzed::v1::{CheckBulkPermissionsRequest, CheckBulkPermissionsResponse};
use tonic::metadata::MetadataValue;
use tonic::service::Interceptor;
use tonic::transport::Channel;
use tonic::{Request, Status};

use super::config::{SpiceDbEndpoint, SpiceDbImportGateBuildError, SpiceDbToken};

#[async_trait]
pub trait BulkImportPermissionCheck: Send + Sync {
    async fn check_bulk_permissions(
        &self,
        request: CheckBulkPermissionsRequest,
    ) -> Result<CheckBulkPermissionsResponse, Status>;
}

#[derive(Clone)]
struct BearerTokenInterceptor {
    token: MetadataValue<tonic::metadata::Ascii>,
}

impl Interceptor for BearerTokenInterceptor {
    fn call(&mut self, mut request: Request<()>) -> Result<Request<()>, Status> {
        request.metadata_mut().insert("authorization", self.token.clone());
        Ok(request)
    }
}

pub struct LiveBulkImportPermissionClient {
    inner: PermissionsServiceClient<tonic::service::interceptor::InterceptedService<Channel, BearerTokenInterceptor>>,
}

impl LiveBulkImportPermissionClient {
    pub async fn connect(
        endpoint: &SpiceDbEndpoint,
        token: &SpiceDbToken,
    ) -> Result<Self, SpiceDbImportGateBuildError> {
        let channel = Channel::from_shared(endpoint.as_str().to_owned())
            .map_err(|e| SpiceDbImportGateBuildError::Connect(e.to_string()))?
            .connect()
            .await
            .map_err(|e| SpiceDbImportGateBuildError::Connect(e.to_string()))?;

        let bearer = format!("Bearer {}", token.expose_secret());
        let metadata = MetadataValue::try_from(bearer).map_err(|e| {
            SpiceDbImportGateBuildError::InvalidToken(format!("authorization metadata invalid: {e}"))
        })?;

        let inner = PermissionsServiceClient::with_interceptor(
            channel,
            BearerTokenInterceptor { token: metadata },
        );

        Ok(Self { inner })
    }
}

#[async_trait]
impl BulkImportPermissionCheck for LiveBulkImportPermissionClient {
    async fn check_bulk_permissions(
        &self,
        request: CheckBulkPermissionsRequest,
    ) -> Result<CheckBulkPermissionsResponse, Status> {
        self.inner
            .clone()
            .check_bulk_permissions(request)
            .await
            .map(|response| response.into_inner())
    }
}
