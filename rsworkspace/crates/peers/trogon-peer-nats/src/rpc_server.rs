use crate::{
    constants::{RPC_SERVICE_DESCRIPTION, RPC_SERVICE_NAME, RPC_SERVICE_VERSION},
    peer_account::PeerAccount,
    rpc_backend::PeerRpcBackend,
    rpc_serve_error::PeerRpcServeError,
};
use grpc_nats_micro::{
    ServiceDescriptor, ServiceName, ServiceNameInput, ServiceVersion, ServiceVersionInput, SubjectPrefix,
    SubjectPrefixInput,
};
use std::sync::Arc;
use trogonai_proto::nats::micro::v1alpha1::{ContentType, ServiceOptions};

pub async fn serve_peer_rpc<B: PeerRpcBackend + 'static>(
    client: &async_nats::Client,
    account: PeerAccount,
    backend: Arc<B>,
) -> Result<async_nats::service::Service, PeerRpcServeError> {
    let prefix = match &account {
        PeerAccount::TenantAccount { .. } => "peer.v1".to_owned(),
        PeerAccount::SharedFixture { tenant } => format!("{}.peer.v1", tenant.as_str()),
    };
    let prefix = SubjectPrefix::from_input(&SubjectPrefixInput::new(prefix.as_str()))?;
    let descriptor = ServiceDescriptor::new(
        ServiceName::from_input(&ServiceNameInput::new(RPC_SERVICE_NAME))?,
        ServiceVersion::from_input(&ServiceVersionInput::new(RPC_SERVICE_VERSION))?,
    )
    .with_description(RPC_SERVICE_DESCRIPTION);
    let mut endpoints = Vec::new();
    for method in crate::peer_method::PeerMethod::ALL {
        endpoints.push(grpc_nats_micro::NamedAdmittedEndpoint {
            method_name: grpc_nats_micro::MethodName::from_input(&grpc_nats_micro::MethodNameInput::new(
                method.as_str(),
            ))?,
            metadata: grpc_nats_micro::DiscoveryMetadata::default(),
            subject: grpc_nats_micro::ServerSubjectPattern::new(&format!(
                "{}.*.{RPC_SERVICE_NAME}.{}",
                prefix.as_str(),
                method.as_str()
            ))?,
            admission: Box::new(crate::rpc_admission::PeerRpcAdmission {
                ingress: crate::peer_ingress::PeerIngress::new(account.clone()),
                method,
                backend: backend.clone(),
            }),
        });
    }
    Ok(grpc_nats_micro::serve_named_admitted(
        client,
        &descriptor,
        ServiceOptions {
            content_type: ContentType::CONTENT_TYPE_PROTOBUF.into(),
            ..Default::default()
        },
        endpoints,
    )
    .await?)
}
