use super::*;

#[derive(Clone)]
enum AckBehavior {
    Fail,
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
}

pub enum AckFuture {
    Fail,
}

impl IntoFuture for AckFuture {
    type Output = Result<PublishAck, MockError>;
    type IntoFuture = std::pin::Pin<Box<dyn Future<Output = Self::Output> + Send>>;

    fn into_future(self) -> Self::IntoFuture {
        match self {
            AckFuture::Fail => Box::pin(async { Err(MockError("simulated ack failure".to_string())) }),
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
        }
    }
}
