use tonic::codegen::http;

use crate::UnixEndpoint;
use crate::constants::MAX_CONTEXT_BYTES;
use crate::workload_request_wire::WorkloadRequestWire;
use crate::workload_response_wire::WorkloadResponseWire;
use crate::workload_source_error::WorkloadSourceError;

pub(crate) async fn connect(
    endpoint: &UnixEndpoint,
) -> Result<tonic::Streaming<WorkloadResponseWire>, WorkloadSourceError> {
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
            http::uri::PathAndQuery::from_static("/SpiffeWorkloadAPI/FetchX509SVID"),
            tonic_prost::ProstCodec::default(),
        )
        .await
        .map_err(Box::new)?;
    Ok(response.into_inner())
}
