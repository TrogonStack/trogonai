use super::Fixture;
use chrono::Duration;
use trogon_peer_service::{ContentBinding, ContentSeal, MessageText, PeerAccess, PeerPortError, PeerService};
use trogonai_proto::peers::v1alpha1::HeartbeatRequest;

pub struct AdvancingSeal<'a> {
    pub fixture: &'a Fixture,
    pub elapsed: Duration,
    pub heartbeat: Option<HeartbeatRequest>,
}

impl ContentSeal for AdvancingSeal<'_> {
    async fn seal(&self, scope: &PeerAccess, text: &MessageText) -> Result<ContentBinding, PeerPortError> {
        *self.fixture.clock.time.lock().map_err(|_| PeerPortError::Unavailable)? += self.elapsed;
        if let Some(request) = &self.heartbeat {
            let service = PeerService::new(
                self.fixture.store.clone(),
                self.fixture.policy.clone(),
                self.fixture.clock.clone(),
            );
            let heartbeat = service.heartbeat(scope.owner(), request.clone()).await;
            assert!(heartbeat.is_ok(), "interleaved heartbeat must succeed: {heartbeat:?}");
        }
        self.fixture.content.seal(scope, text).await
    }
}
