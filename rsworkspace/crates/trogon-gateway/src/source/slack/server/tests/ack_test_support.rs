use super::*;
use async_nats::jetstream::publish::PublishAck;
use std::sync::Arc;
use std::sync::Mutex;
use trogon_nats::mocks::MockError;

#[derive(Clone)]
enum AckBehavior {
    Fail,
    Hang,
}

#[derive(Clone)]
pub struct AckFailPublisher {
    behavior: Arc<Mutex<AckBehavior>>,
}

impl AckFailPublisher {
    pub fn failing() -> Self {
        Self {
            behavior: Arc::new(Mutex::new(AckBehavior::Fail)),
        }
    }

    pub fn hanging() -> Self {
        Self {
            behavior: Arc::new(Mutex::new(AckBehavior::Hang)),
        }
    }
}

pub enum AckFuture {
    Fail,
    Hang,
}

impl IntoFuture for AckFuture {
    type Output = Result<PublishAck, MockError>;
    type IntoFuture = std::pin::Pin<Box<dyn Future<Output = Self::Output> + Send>>;

    fn into_future(self) -> Self::IntoFuture {
        match self {
            AckFuture::Fail => Box::pin(async { Err(MockError("simulated ack failure".to_string())) }),
            AckFuture::Hang => Box::pin(std::future::pending()),
        }
    }
}

impl JetStreamPublisher for AckFailPublisher {
    type PublishError = MockError;
    type AckFuture = AckFuture;

    async fn publish_with_headers<S: ToSubject + Send>(
        &self,
        _subject: S,
        _headers: async_nats::HeaderMap,
        _payload: Bytes,
    ) -> Result<AckFuture, MockError> {
        let behavior = self.behavior.lock().unwrap().clone();
        match behavior {
            AckBehavior::Fail => Ok(AckFuture::Fail),
            AckBehavior::Hang => Ok(AckFuture::Hang),
        }
    }
}
