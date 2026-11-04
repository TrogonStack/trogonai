use serde::Serialize;
use trogonai_proto::peers::v1alpha1::MessageStatus;

use crate::message_metadata_projection::MessageMetadataProjection;

#[derive(Serialize)]
pub(crate) struct StatusMetadataProjection {
    #[serde(flatten)]
    pub(crate) message: MessageMetadataProjection,
    pub(crate) status: MessageStatus,
    pub(crate) observed_at: String,
}
