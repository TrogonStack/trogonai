use std::error::Error;
use std::sync::Mutex;
use std::time::Duration;

use super::*;

type TestResult = Result<(), Box<dyn Error>>;

#[derive(Default)]
struct RecordingBus {
    calls: Mutex<Vec<&'static str>>,
}

impl RecordingBus {
    fn note(&self, call: &'static str) {
        self.calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(call);
    }

    fn calls(&self) -> Vec<&'static str> {
        self.calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

#[async_trait]
impl BrokerBus for RecordingBus {
    async fn request(&self, _subject: &str, payload: Bytes) -> anyhow::Result<Bytes> {
        self.note("request");
        Ok(payload)
    }

    async fn publish(&self, _subject: &str, _payload: Bytes) -> anyhow::Result<()> {
        self.note("publish");
        Ok(())
    }
}

#[tokio::test]
async fn defaults_fall_back_to_plain_publish() -> TestResult {
    let bus = RecordingBus::default();
    bus.publish_with_msg_id("s", "id", Bytes::new()).await?;
    bus.publish_with_auth("s", "auth", Bytes::new()).await?;
    assert_eq!(bus.calls(), vec!["publish", "publish"]);
    assert_eq!(bus.max_payload(), None);
    Ok(())
}

#[tokio::test]
async fn defaults_fall_back_to_plain_request() -> TestResult {
    let bus = RecordingBus::default();
    let payload = Bytes::from_static(b"x");
    assert_eq!(
        bus.request_with_timeout("s", payload.clone(), Duration::from_secs(1))
            .await?,
        payload
    );
    assert_eq!(
        bus.request_with_auth("s", "auth", payload.clone(), None).await?,
        payload
    );
    assert_eq!(
        bus.request_with_auth("s", "auth", payload.clone(), Some(Duration::from_secs(1)))
            .await?,
        payload
    );
    assert_eq!(bus.calls(), vec!["request", "request", "request"]);
    Ok(())
}
