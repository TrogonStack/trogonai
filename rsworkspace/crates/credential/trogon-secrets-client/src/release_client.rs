use grpc_nats_micro::client::request;
use grpc_nats_micro::{
    ContentType, DiscoveryMetadata, EndpointBinding, MethodName, MethodNameInput, ServiceName, ServiceNameInput,
    SubjectPrefix, SubjectPrefixInput,
};
use nkeys::XKey;
use trogon_nats::RequestClient;
use trogon_std::time::EpochClock;
use trogonai_proto::secrets::exchange::v1alpha1::ResolveForReleaseResponse;
use trogonai_proto::secrets::exchange::v1alpha1::resolve_for_release_response::Result as Outcome;

use super::constants::{METHOD_NAME, SERVICE_NAME, SUBJECT_DOMAIN};
use super::{
    InvalidReleaseResponseError, ReleaseClientConfig, ReleaseError, ReleaseRequest, ReleaseSubjectError, ReleasedValue,
    TenantAccount,
};

/// Asks the secrets service for one credential release at a time over
/// `SecretExchangeService.ResolveForRelease`.
///
/// It calls inside PLATFORM, on
/// `{tenant}.secrets.v1.service.{id}.SecretExchangeService.ResolveForRelease`,
/// with the tenant from the request and the runtime service from its
/// configuration. Each call seals to an xkey of its own, made for that call
/// and dropped with it, and opens the answer only with the pinned service
/// xkey as the sender.
#[derive(Clone)]
pub struct ReleaseClient<N, C> {
    nats: N,
    clock: C,
    config: ReleaseClientConfig,
}

impl<N, C> ReleaseClient<N, C>
where
    N: RequestClient,
    N::RequestError: 'static,
    C: EpochClock,
{
    pub fn new(nats: N, clock: C, config: ReleaseClientConfig) -> Self {
        Self { nats, clock, config }
    }

    pub fn config(&self) -> &ReleaseClientConfig {
        &self.config
    }

    /// Asks for `release` and opens the value the service sealed to this
    /// call, or says why there is none.
    pub async fn release(&self, release: &ReleaseRequest) -> Result<ReleasedValue, ReleaseError> {
        let endpoint = self.endpoint(release.tenant())?;
        let requester = XKey::new();
        let response: ResolveForReleaseResponse = request(
            &self.nats,
            &endpoint,
            ContentType::Protobuf,
            &release.to_wire(requester.public_key()),
            self.config.timeout().get(),
        )
        .await
        .map_err(ReleaseError::from_request)?;
        match response.result.ok_or(InvalidReleaseResponseError::MissingResult)? {
            Outcome::Access(access) => ReleasedValue::open(
                *access,
                &requester,
                self.config.service_xkey(),
                self.clock.system_time(),
            ),
            Outcome::Failure(failure) => Err(ReleaseError::reported(*failure)?),
        }
    }

    fn endpoint(&self, tenant: &TenantAccount) -> Result<EndpointBinding, ReleaseSubjectError> {
        let prefix = format!("{tenant}.{SUBJECT_DOMAIN}.{}", self.config.runtime_service());
        let prefix =
            SubjectPrefix::from_input(&SubjectPrefixInput::new(prefix)).map_err(ReleaseSubjectError::Prefix)?;
        let service =
            ServiceName::from_input(&ServiceNameInput::new(SERVICE_NAME)).map_err(ReleaseSubjectError::Service)?;
        let method = MethodName::from_input(&MethodNameInput::new(METHOD_NAME)).map_err(ReleaseSubjectError::Method)?;
        EndpointBinding::new(&prefix, &service, method, DiscoveryMetadata::default())
            .map_err(ReleaseSubjectError::Endpoint)
    }
}

#[cfg(test)]
mod tests;
