use crate::{
    constants::{RPC_SERVICE_DESCRIPTION, RPC_SERVICE_NAME, RPC_SERVICE_VERSION},
    peer_account::PeerAccount,
    rpc_backend::PeerRpcBackend,
    rpc_serve_error::PeerRpcServeError,
};
use grpc_nats_micro::{
    ServiceBinding, ServiceName, ServiceNameInput, ServiceVersion, ServiceVersionInput, SubjectPrefix,
    SubjectPrefixInput,
};
use std::sync::Arc;
use trogonai_proto::nats::micro::v1alpha1::{ContentType, ServiceOptions};

pub async fn serve_peer_rpc<B: PeerRpcBackend + 'static>(
    client: &async_nats::Client,
    account: PeerAccount,
    _backend: Arc<B>,
) -> Result<async_nats::service::Service, PeerRpcServeError> {
    let prefix = match &account {
        PeerAccount::TenantAccount { .. } => "peer.v1".to_owned(),
        PeerAccount::SharedFixture { tenant } => format!("{}.peer.v1", tenant.as_str()),
    };
    let binding = ServiceBinding::new(
        ServiceName::from_input(&ServiceNameInput::new(RPC_SERVICE_NAME))?,
        ServiceVersion::from_input(&ServiceVersionInput::new(RPC_SERVICE_VERSION))?,
        SubjectPrefix::from_input(&SubjectPrefixInput::new(prefix))?,
    )
    .with_description(RPC_SERVICE_DESCRIPTION);
    Ok(grpc_nats_micro::serve_admitted(
        client,
        &binding,
        ServiceOptions {
            content_type: ContentType::CONTENT_TYPE_PROTOBUF.into(),
            ..Default::default()
        },
        Vec::new(),
    )
    .await?)
}
