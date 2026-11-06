use trogon_workload_registration::{ApprovedRegistration, ProducerConfig};

use super::runner_error::RunnerError;

pub(super) struct SignedFrameWire {
    pub signature: [u8; 64],
    pub payload: Vec<u8>,
}

impl SignedFrameWire {
    pub fn verify(self, config: &ProducerConfig) -> Result<ApprovedRegistration, RunnerError> {
        ApprovedRegistration::verify(config, &self.payload, &self.signature).map_err(RunnerError::approval)
    }
}
