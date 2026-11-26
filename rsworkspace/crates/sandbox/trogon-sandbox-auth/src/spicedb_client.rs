use std::fmt;

use authzed::v1::check_permission_response::Permissionship;
use authzed::v1::permissions_service_client::PermissionsServiceClient;
use authzed::v1::{CheckPermissionRequest, ObjectReference, SubjectReference};
use tonic::metadata::{Ascii, MetadataValue};
use tonic::service::Interceptor;
use tonic::service::interceptor::InterceptedService;
use tonic::transport::Channel;
use tonic::{Code, Request, Status};
use trogon_sandbox_contract::{ServiceAccountId, TenantId};

use crate::{AuthzError, SpiceDbConnectError, SpiceDbEndpoint, SpiceDbToken, TenantResourceType};

const SUBJECT_TYPE: &str = "service_account";
const PERMISSION_LEASE_SANDBOX: &str = "lease_sandbox";

#[derive(Clone)]
struct BearerInterceptor {
    authorization: MetadataValue<Ascii>,
}

impl Interceptor for BearerInterceptor {
    fn call(&mut self, mut request: Request<()>) -> Result<Request<()>, Status> {
        request
            .metadata_mut()
            .insert("authorization", self.authorization.clone());
        Ok(request)
    }
}

/// A gRPC client for the permission checks the sandbox services make.
#[derive(Clone)]
pub struct SpiceDbClient {
    inner: PermissionsServiceClient<InterceptedService<Channel, BearerInterceptor>>,
    tenant_resource_type: TenantResourceType,
}

impl SpiceDbClient {
    /// Connects to `endpoint`, checking tenants as the default [`TenantResourceType`].
    pub async fn connect(endpoint: &SpiceDbEndpoint, token: &SpiceDbToken) -> Result<Self, SpiceDbConnectError> {
        Self::connect_with_resource_type(endpoint, token, TenantResourceType::default()).await
    }

    pub async fn connect_with_resource_type(
        endpoint: &SpiceDbEndpoint,
        token: &SpiceDbToken,
        tenant_resource_type: TenantResourceType,
    ) -> Result<Self, SpiceDbConnectError> {
        let authorization = token.authorization()?;
        let channel = endpoint.to_endpoint().connect().await?;
        Ok(Self {
            inner: PermissionsServiceClient::with_interceptor(channel, BearerInterceptor { authorization }),
            tenant_resource_type,
        })
    }

    /// Checks that `service_account` holds the sandbox lease permission on `tenant`.
    ///
    /// A missing permission or an unknown subject is [`AuthzError::Denied`]; any other failure is
    /// [`AuthzError::Backend`].
    pub async fn check_sandbox_lease(
        &self,
        tenant: &TenantId,
        service_account: &ServiceAccountId,
    ) -> Result<(), AuthzError> {
        let request = CheckPermissionRequest {
            consistency: None,
            resource: Some(ObjectReference {
                object_type: self.tenant_resource_type.to_string(),
                object_id: tenant.to_string(),
            }),
            permission: PERMISSION_LEASE_SANDBOX.to_string(),
            subject: Some(SubjectReference {
                object: Some(ObjectReference {
                    object_type: SUBJECT_TYPE.to_string(),
                    object_id: service_account.to_string(),
                }),
                optional_relation: String::new(),
            }),
            context: None,
            with_tracing: false,
        };
        let response = self
            .inner
            .clone()
            .check_permission(request)
            .await
            .map_err(|status| match status.code() {
                Code::PermissionDenied | Code::NotFound => AuthzError::Denied,
                _ => AuthzError::backend(status),
            })?;
        if response.into_inner().permissionship == Permissionship::HasPermission as i32 {
            Ok(())
        } else {
            Err(AuthzError::Denied)
        }
    }
}

impl fmt::Debug for SpiceDbClient {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SpiceDbClient")
            .field("tenant_resource_type", &self.tenant_resource_type)
            .finish_non_exhaustive()
    }
}
