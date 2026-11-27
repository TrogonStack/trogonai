use std::sync::Arc;

use crate::constants::{ISSUE_SUBJECT, REVOKE_SUBJECT, ROTATE_SUBJECT};
use crate::{BrokerService, ServiceError, ServiceSubscriptions};

impl BrokerService {
    /// Subscribes to the request subjects and waits until the server has them, so a request sent afterwards is
    /// never answered with no responders.
    pub async fn subscribe(self: Arc<Self>, client: async_nats::Client) -> Result<ServiceSubscriptions, ServiceError> {
        let issue = client.subscribe(ISSUE_SUBJECT).await?;
        let revoke = client.subscribe(REVOKE_SUBJECT).await?;
        let rotate = client.subscribe(ROTATE_SUBJECT).await?;
        client.flush().await?;
        Ok(ServiceSubscriptions::new(self, client, issue, revoke, rotate))
    }
}
