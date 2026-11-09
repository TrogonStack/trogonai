use std::sync::atomic::{AtomicUsize, Ordering};

use tokio::sync::Notify;
use tracing::instrument::WithSubscriber;
use tracing_subscriber::Layer;
use tracing_subscriber::layer::{Context, SubscriberExt};

use super::*;
use crate::AdmissionLimit;
use crate::constants::ADMISSION_TEST_WAIT;

struct ErrorEvents(Arc<AtomicUsize>);

impl<S: tracing::Subscriber> Layer<S> for ErrorEvents {
    fn on_event(&self, event: &tracing::Event<'_>, _: Context<'_, S>) {
        if *event.metadata().level() == tracing::Level::ERROR {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
}

struct CountingDispatcher(Arc<AtomicUsize>);

#[async_trait::async_trait]
impl AuthDispatcher for CountingDispatcher {
    async fn dispatch(&self, _: ServerAuthRequestClaims) -> Result<MintedUserJwt, AuthCalloutError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(jwt("must-not-dispatch"))
    }
}

#[tokio::test]
async fn a_buffered_overload_on_a_closed_connection_reports_publish_failure_without_dispatch() {
    let server = CoreTestServer::start().await;
    let closed = Arc::new(Notify::new());
    let observed_closed = Arc::clone(&closed);
    let client = async_nats::ConnectOptions::new()
        .event_callback(move |event| {
            let closed = Arc::clone(&observed_closed);
            async move {
                if matches!(event, async_nats::Event::Closed) {
                    closed.notify_one();
                }
            }
        })
        .connect(server.address())
        .await
        .unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let budget = AdmissionBudget::new(AdmissionLimit::new(1).unwrap());
    let _held = budget.try_acquire().unwrap();
    let subscriber = BridgeMintSubscriber::subscribe(
        client.clone(),
        BridgeMintResponder::new(CountingDispatcher(Arc::clone(&calls)), None),
        &InMemoryEnv::new(),
    )
    .await
    .unwrap()
    .with_admission_budget(budget);
    let mut observed = client.subscribe(DEFAULT_BRIDGE_MINT_SUBJECT).await.unwrap();
    client.flush().await.unwrap();
    let publisher = async_nats::connect(server.address()).await.unwrap();
    publisher
        .publish_with_reply(DEFAULT_BRIDGE_MINT_SUBJECT, publisher.new_inbox(), Vec::new().into())
        .await
        .unwrap();
    publisher.flush().await.unwrap();
    tokio::time::timeout(ADMISSION_TEST_WAIT, observed.next())
        .await
        .unwrap()
        .expect("the real broker must deliver the buffered request before drain");
    client.drain().await.unwrap();
    tokio::time::timeout(ADMISSION_TEST_WAIT, closed.notified())
        .await
        .unwrap();
    client
        .flush()
        .await
        .expect_err("the responder connection must be fully closed");

    let errors = Arc::new(AtomicUsize::new(0));
    let diagnostics = tracing_subscriber::registry().with(ErrorEvents(Arc::clone(&errors)));
    tokio::time::timeout(ADMISSION_TEST_WAIT, subscriber.serve().with_subscriber(diagnostics))
        .await
        .expect("buffered overload and closed subscription must settle");
    assert_eq!(errors.load(Ordering::SeqCst), 1);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    publisher.drain().await.unwrap();
}
