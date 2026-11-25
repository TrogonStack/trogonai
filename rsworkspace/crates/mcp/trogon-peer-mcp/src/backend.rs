use std::error::Error;
use std::future::Future;

use trogonai_proto::peers::v1alpha1::{EnrollRequest, EnrollResponse, PrepareBatchRequest, PrepareBatchResponse};

pub trait EnrollPeer: Send + Sync + 'static {
    type Error: Error + Send + Sync + 'static;

    fn enroll(&self, request: EnrollRequest) -> impl Future<Output = Result<EnrollResponse, Self::Error>> + Send;
}

pub trait PreparePeerBatch: Send + Sync + 'static {
    type Error: Error + Send + Sync + 'static;

    fn prepare_batch(
        &self,
        request: PrepareBatchRequest,
    ) -> impl Future<Output = Result<PrepareBatchResponse, Self::Error>> + Send;
}
