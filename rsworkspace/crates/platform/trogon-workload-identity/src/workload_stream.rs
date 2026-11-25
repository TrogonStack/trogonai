use tonic::codegen::http;

use crate::UnixEndpoint;
use crate::constants::{FETCH_JWT_BUNDLES, FETCH_X509_SVID, MAX_CONTEXT_BYTES};
use crate::jwt_bundles_wire::JwtBundlesWire;
use crate::workload_request_wire::WorkloadRequestWire;
use crate::workload_response_wire::WorkloadResponseWire;
use crate::workload_source_error::WorkloadSourceError;

pub(crate) async fn connect(
    endpoint: &UnixEndpoint,
) -> Result<tonic::Streaming<WorkloadResponseWire>, WorkloadSourceError> {
    open(endpoint, FETCH_X509_SVID).await
}

pub(crate) async fn connect_jwt_bundles(
    endpoint: &UnixEndpoint,
) -> Result<tonic::Streaming<JwtBundlesWire>, WorkloadSourceError> {
    open(endpoint, FETCH_JWT_BUNDLES).await
}

async fn open<R>(endpoint: &UnixEndpoint, path: &'static str) -> Result<tonic::Streaming<R>, WorkloadSourceError>
where
    R: prost::Message + Default + Send + 'static,
{
    let channel = spiffe::transport::connect(&endpoint.endpoint()).await?;
    let mut client = tonic::client::Grpc::new(channel).max_decoding_message_size(MAX_CONTEXT_BYTES);
    client.ready().await?;
    let mut request = tonic::Request::new(WorkloadRequestWire {});
    request.metadata_mut().insert(
        "workload.spiffe.io",
        tonic::metadata::MetadataValue::from_static("true"),
    );
    let response = client
        .server_streaming(
            request,
            http::uri::PathAndQuery::from_static(path),
            tonic_prost::ProstCodec::default(),
        )
        .await
        .map_err(Box::new)?;
    Ok(response.into_inner())
}
