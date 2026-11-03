use crate::{EndpointHandler, request_metadata::RequestMetadata};

pub trait EndpointAdmission: Send + Sync {
    fn admit(&self, metadata: RequestMetadata<'_>) -> Option<Box<dyn EndpointHandler + '_>>;
}
