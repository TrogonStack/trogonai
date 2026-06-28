//! Bulk-import permission-check trait + the tonic-backed live wrapper
//! that calls SpiceDB over gRPC.
//!
//! Tests construct their own fake `BulkImportPermissionCheck` impls;
//! the live `LiveBulkImportPermissionClient` lives behind the
//! `spicedb` feature alongside the rest of the authzed/tonic chain.

use async_trait::async_trait;
use authzed::v1::permissions_service_client::PermissionsServiceClient;
use authzed::v1::{
    CheckBulkPermissionsRequest, CheckBulkPermissionsResponse, WriteRelationshipsRequest, WriteRelationshipsResponse,
};
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

    async fn write_relationships(
        &self,
        request: WriteRelationshipsRequest,
    ) -> Result<WriteRelationshipsResponse, Status>;
}

/// Per-request interceptor that attaches `Authorization: Bearer <token>`
/// metadata. The token is stored as a pre-validated `MetadataValue`
/// so we can't get a "value contains invalid characters" failure on
/// the hot path -- that error has to surface at construction time.
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

/// Tonic-backed gRPC client that satisfies [`BulkImportPermissionCheck`].
///
/// Construction goes through [`Self::connect`] so the endpoint, channel
/// dial, and bearer-token metadata are all validated up front --
/// production callsites can rely on a successfully-constructed client
/// being ready to issue RPCs without per-call retry-on-config-error
/// branching.
pub struct LiveBulkImportPermissionClient {
    inner: PermissionsServiceClient<tonic::service::interceptor::InterceptedService<Channel, BearerTokenInterceptor>>,
}

impl std::fmt::Debug for LiveBulkImportPermissionClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // `PermissionsServiceClient` and `Channel` don't implement
        // Debug; emit a constructed-marker so logs and test
        // assertions on `Result::expect_err` have something
        // human-readable without leaking the bearer token.
        f.debug_struct("LiveBulkImportPermissionClient")
            .field("transport", &"<tonic+grpc>")
            .finish()
    }
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
        let metadata = MetadataValue::try_from(bearer)
            .map_err(|e| SpiceDbImportGateBuildError::InvalidToken(format!("authorization metadata invalid: {e}")))?;

        let inner = PermissionsServiceClient::with_interceptor(channel, BearerTokenInterceptor { token: metadata });

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

    async fn write_relationships(
        &self,
        request: WriteRelationshipsRequest,
    ) -> Result<WriteRelationshipsResponse, Status> {
        self.inner
            .clone()
            .write_relationships(request)
            .await
            .map(|response| response.into_inner())
    }
}

#[cfg(test)]
mod tests;
