use trogon_sandbox_contract::{
    CapabilityProfile, GetLeaseByNameRequest, GetLeaseByNameResponse, GetSandboxDomainRequest,
    GetSandboxDomainResponse, LeaseForkRequest, LeaseGranted, LeaseId, LeaseName, LeaseResponse,
    ListCapabilitiesRequest, ListCapabilitiesResponse, ListLeasesRequest, ListLeasesResponse, NativeHandleRequest,
    NativeHandleResponse, ProviderKind, ReapLeasesRequest, ReapLeasesResponse, SshAccessRequest, SshAccessResponse,
    TenantId,
};
use trogon_sandbox_protocol::subjects;

use crate::constants::{FORK_TIMEOUT, LISTING_TIMEOUT, REAP_TIMEOUT};
use crate::{ClientError, SandboxClient};

impl SandboxClient {
    /// Forks a currently running lease into a brand-new, independent lease seeded from its live state. The source
    /// lease keeps running untouched.
    pub async fn fork(
        &self,
        tenant: TenantId,
        source_lease_id: &LeaseId,
        ttl_seconds: u32,
        idempotency_key: String,
    ) -> Result<LeaseGranted, ClientError> {
        let request = LeaseForkRequest {
            tenant,
            source_lease_id: source_lease_id.clone(),
            ttl_seconds,
            idempotency_key,
        };
        let response: LeaseResponse = self.send(subjects::LEASE_FORK_REQUEST, &request, FORK_TIMEOUT).await?;
        match response {
            LeaseResponse::Granted(granted) => Ok(granted),
            LeaseResponse::Denied(error) => Err(ClientError::Denied(error)),
        }
    }

    /// Resolves `port` from `lease_id`'s exposed ports into a reachable endpoint.
    pub async fn get_sandbox_domain(
        &self,
        lease_id: &LeaseId,
        port: u16,
    ) -> Result<GetSandboxDomainResponse, ClientError> {
        let request = GetSandboxDomainRequest {
            lease_id: lease_id.clone(),
            port,
            wait_ms: None,
        };
        let response: GetSandboxDomainResponse = self
            .send_default(&subjects::lease_get_domain(lease_id), &request)
            .await?;
        match response {
            GetSandboxDomainResponse::Denied(error) => Err(ClientError::Denied(error)),
            other => Ok(other),
        }
    }

    /// The vendor's own handle (sandbox id and, when it has one, a URL) for `lease_id`, so a caller can reach the
    /// sandbox with the vendor's SDK. Needs the tenant to be allowed native passthrough; a provider with no such
    /// handle answers `Unsupported`.
    pub async fn native_handle(&self, lease_id: &LeaseId) -> Result<NativeHandleResponse, ClientError> {
        let request = NativeHandleRequest {
            lease_id: lease_id.clone(),
        };
        let response: NativeHandleResponse = self
            .send_default(&subjects::lease_native_handle(lease_id), &request)
            .await?;
        match response {
            NativeHandleResponse::Denied(error) => Err(ClientError::Denied(error)),
            other => Ok(other),
        }
    }

    /// Direct SSH access (endpoint plus credential) to `lease_id`'s sandbox, valid for about `ttl_seconds` when the
    /// provider honours a lifetime. Needs the tenant to be allowed native passthrough; a provider with no SSH
    /// endpoint answers `Unsupported`.
    pub async fn ssh_access(
        &self,
        lease_id: &LeaseId,
        ttl_seconds: Option<u32>,
    ) -> Result<SshAccessResponse, ClientError> {
        let request = SshAccessRequest {
            lease_id: lease_id.clone(),
            ttl_seconds,
        };
        let response: SshAccessResponse = self
            .send_default(&subjects::lease_ssh_access(lease_id), &request)
            .await?;
        match response {
            SshAccessResponse::Denied(error) => Err(ClientError::Denied(error)),
            other => Ok(other),
        }
    }

    /// Looks up a running lease's row by its tenant-scoped `name`.
    pub async fn get_lease_by_name(
        &self,
        tenant: TenantId,
        name: LeaseName,
    ) -> Result<GetLeaseByNameResponse, ClientError> {
        let request = GetLeaseByNameRequest { tenant, name };
        let response: GetLeaseByNameResponse = self.send_default(subjects::LEASE_GET_BY_NAME_REQUEST, &request).await?;
        match response {
            GetLeaseByNameResponse::Denied(error) => Err(ClientError::Denied(error)),
            other => Ok(other),
        }
    }

    /// Lists a tenant's leases. A refusal is [`ClientError::Denied`], never an empty page.
    pub async fn list_leases(&self, request: ListLeasesRequest) -> Result<ListLeasesResponse, ClientError> {
        let mut response: ListLeasesResponse = self
            .send(subjects::LEASE_LIST_REQUEST, &request, LISTING_TIMEOUT)
            .await?;
        match response.denied.take() {
            Some(error) => Err(ClientError::Denied(error)),
            None => Ok(response),
        }
    }

    /// Releases the tenant's leases matching the tag, age and idleness filters in `request`, or only reports them
    /// when the request is a dry run. Repeating the call is safe: leases already released no longer match.
    pub async fn reap_leases(&self, request: ReapLeasesRequest) -> Result<ReapLeasesResponse, ClientError> {
        let response: ReapLeasesResponse = self.send(subjects::LEASE_REAP_REQUEST, &request, REAP_TIMEOUT).await?;
        match response {
            ReapLeasesResponse::Denied(error) => Err(ClientError::Denied(error)),
            other => Ok(other),
        }
    }

    /// Lists provider capability profiles from the broker's worker registry, optionally filtered to a single
    /// `provider`.
    pub async fn list_capabilities(
        &self,
        provider: Option<ProviderKind>,
    ) -> Result<Vec<CapabilityProfile>, ClientError> {
        let request = ListCapabilitiesRequest { provider };
        let response: ListCapabilitiesResponse = self
            .send(subjects::CAPABILITY_LIST_REQUEST, &request, LISTING_TIMEOUT)
            .await?;
        Ok(response.profiles)
    }
}
