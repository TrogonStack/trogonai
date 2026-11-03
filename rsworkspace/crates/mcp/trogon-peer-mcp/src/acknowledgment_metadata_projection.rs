use serde::Serialize;

use crate::application_acknowledgment::ApplicationAcknowledgment;

#[derive(Serialize)]
pub(crate) struct AcknowledgmentMetadataProjection {
    pub(crate) batch_id: String,
    pub(crate) acknowledgment: ApplicationAcknowledgment,
}
