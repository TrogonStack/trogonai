use async_nats::Client;

use super::errors::AnomalyError;
use super::features::AnomalyFeatures;

pub fn subject_for_tenant(tenant_id: &str) -> String {
    format!("mcp.anomaly.features.{tenant_id}")
}

#[derive(Clone)]
pub struct AnomalyEmitter {
    client: Client,
}

impl AnomalyEmitter {
    #[must_use]
    pub fn new(client: Client) -> Self {
        Self { client }
    }

    pub async fn emit(&self, features: &AnomalyFeatures) -> Result<(), AnomalyError> {
        let subject = subject_for_tenant(&features.tenant_id);
        let payload =
            serde_json::to_vec(features).map_err(AnomalyError::SerializeFailed)?;
        self.client
            .publish(subject, payload.into())
            .await
            .map_err(|err| AnomalyError::TransportFailed(err.to_string()))?;
        self.client
            .flush()
            .await
            .map_err(|err| AnomalyError::TransportFailed(err.to_string()))?;
        Ok(())
    }
}
