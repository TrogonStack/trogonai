use super::*;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

const WINDOW: Duration = Duration::from_secs(10);

/// How one send of the request ends.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Outcome {
    Replied,
    NoResponders,
    TimedOut,
    Other,
}

#[derive(Debug, thiserror::Error)]
#[error("{0:?}")]
struct ScriptedError(Outcome);

/// A client whose sends end as scripted, in order (the last one repeats), and
/// that records when each send happened.
#[derive(Clone)]
struct ScriptedNats {
    outcomes: Arc<Mutex<VecDeque<Outcome>>>,
    sends: Arc<Mutex<Vec<Instant>>>,
    connected: Arc<AtomicBool>,
}

impl ScriptedNats {
    fn new(outcomes: impl IntoIterator<Item = Outcome>) -> Self {
        Self {
            outcomes: Arc::new(Mutex::new(outcomes.into_iter().collect())),
            sends: Arc::default(),
            connected: Arc::new(AtomicBool::new(true)),
        }
    }

    fn disconnected(self) -> Self {
        self.connected.store(false, Ordering::SeqCst);
        self
    }

    fn reconnect_after(&self, delay: Duration) -> impl Future<Output = ()> + use<> {
        let connected = self.connected.clone();
        async move {
            tokio::time::sleep(delay).await;
            connected.store(true, Ordering::SeqCst);
        }
    }

    fn sends(&self) -> Vec<Instant> {
        self.sends.lock().unwrap().clone()
    }
}

impl RequestClient for ScriptedNats {
    type RequestError = ScriptedError;

    async fn request_with_headers<S: ToSubject + Send>(
        &self,
        subject: S,
        _headers: HeaderMap,
        payload: Bytes,
    ) -> Result<Message, Self::RequestError> {
        self.sends.lock().unwrap().push(Instant::now());
        let outcome = {
            let mut outcomes = self.outcomes.lock().unwrap();
            if outcomes.len() > 1 {
                outcomes.pop_front()
            } else {
                outcomes.front().copied()
            }
        };
        match outcome.expect("scripted outcome") {
            Outcome::Replied => Ok(Message {
                subject: subject.to_subject(),
                reply: None,
                length: payload.len(),
                payload,
                headers: None,
                status: None,
                description: None,
            }),
            failure => Err(ScriptedError(failure)),
        }
    }
}

impl ConnectionAware for ScriptedNats {
    fn is_connected(&self) -> bool {
        self.connected.load(Ordering::SeqCst)
    }

    fn is_no_responders(error: &ScriptedError) -> bool {
        error.0 == Outcome::NoResponders
    }
}

async fn request(nats: &OutageTolerantNats<ScriptedNats>) -> Result<Message, ScriptedError> {
    nats.request_with_headers(
        "acp.v1.session.s1.client.session.request_permission",
        HeaderMap::new(),
        Bytes::from_static(b"ask"),
    )
    .await
}

#[tokio::test(start_paused = true)]
async fn no_responders_is_sent_again_with_backoff_until_the_client_answers() {
    let inner = ScriptedNats::new([Outcome::NoResponders, Outcome::NoResponders, Outcome::Replied]);
    let nats = OutageTolerantNats::new(inner.clone(), WINDOW);
    let start = Instant::now();

    let reply = request(&nats).await.expect("answered once the client is back");

    assert_eq!(&reply.payload[..], b"ask");
    let gaps: Vec<_> = inner.sends().iter().map(|sent| *sent - start).collect();
    assert_eq!(
        gaps,
        [Duration::ZERO, RETRY_BACKOFF_START, RETRY_BACKOFF_START * 3],
        "sent at once, then after 100ms, then 200ms more"
    );
}

#[tokio::test(start_paused = true)]
async fn any_other_failure_is_returned_at_once() {
    // A timeout above all: the client may have the request and still be
    // working on it, so sending it again could run it twice.
    for failure in [Outcome::TimedOut, Outcome::Other] {
        let inner = ScriptedNats::new([failure, Outcome::Replied]);
        let nats = OutageTolerantNats::new(inner.clone(), WINDOW);

        assert_eq!(request(&nats).await.expect_err("not sent again").0, failure);
        assert_eq!(inner.sends().len(), 1, "{failure:?} is not sent again");
    }
}

#[tokio::test(start_paused = true)]
async fn no_responders_for_the_whole_window_fails_inside_it() {
    let inner = ScriptedNats::new([Outcome::NoResponders]);
    let nats = OutageTolerantNats::new(inner.clone(), WINDOW);
    let start = Instant::now();

    let error = request(&nats).await.expect_err("nobody ever subscribed");

    assert_eq!(error.0, Outcome::NoResponders);
    let sends = inner.sends();
    assert!(Instant::now() - start < WINDOW, "gave up before the window ended");
    // 0, 0.1, 0.3, 0.7, 1.5, 2.5, ... 9.5: backoff doubles to 1s and then stays.
    assert_eq!(sends.len(), 13);
    assert!(sends.windows(2).all(|pair| pair[1] - pair[0] <= RETRY_BACKOFF_MAX));
}

#[tokio::test(start_paused = true)]
async fn a_zero_window_sends_once() {
    let inner = ScriptedNats::new([Outcome::NoResponders, Outcome::Replied]);
    let nats = OutageTolerantNats::new(inner.clone(), Duration::ZERO);

    assert_eq!(request(&nats).await.expect_err("no retry").0, Outcome::NoResponders);
    assert_eq!(inner.sends().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn a_request_waits_for_the_connection_before_it_is_sent() {
    let inner = ScriptedNats::new([Outcome::Replied]).disconnected();
    let nats = OutageTolerantNats::new(inner.clone(), WINDOW);
    let start = Instant::now();
    let reconnect = inner.reconnect_after(Duration::from_secs(3));

    let (reply, ()) = tokio::join!(request(&nats), reconnect);

    reply.expect("answered after the reconnect");
    let sends = inner.sends();
    assert_eq!(sends.len(), 1);
    assert!(
        sends[0] - start >= Duration::from_secs(3),
        "not sent while disconnected"
    );
}

#[tokio::test(start_paused = true)]
async fn a_request_still_disconnected_when_the_window_ends_is_sent_anyway() {
    let inner = ScriptedNats::new([Outcome::Replied]).disconnected();
    let nats = OutageTolerantNats::new(inner.clone(), WINDOW);
    let start = Instant::now();

    request(&nats).await.expect("the client queues it for the reconnect");

    let sends = inner.sends();
    assert_eq!(sends.len(), 1);
    assert!(sends[0] - start >= WINDOW);
}

#[tokio::test(start_paused = true)]
async fn the_retry_window_starts_once_the_host_is_connected() {
    let inner = ScriptedNats::new([Outcome::NoResponders]).disconnected();
    let nats = OutageTolerantNats::new(inner.clone(), WINDOW);
    let start = Instant::now();
    let reconnect = inner.reconnect_after(Duration::from_secs(3));

    let (error, ()) = tokio::join!(request(&nats), reconnect);

    assert_eq!(error.expect_err("nobody ever subscribed").0, Outcome::NoResponders);
    let sends = inner.sends();
    assert!(sends[0] - start >= Duration::from_secs(3) && sends[0] - start <= Duration::from_secs(3) + CONNECTED_POLL);
    assert_eq!(*sends.last().unwrap() - sends[0], Duration::from_millis(9500));
}

mod nats_restart;
