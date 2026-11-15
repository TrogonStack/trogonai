use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::future::{Ready, pending, ready};
use std::time::Duration;

use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use trogon_secret_store::{OpenBaoLoginError, OpenBaoSecretStoreConfigError};
use trogon_std::log_capture::{CapturedEvent, CapturedEvents, LevelFilter};

use super::LoginRetry;

const INITIAL: Duration = Duration::from_secs(1);
const MAX: Duration = Duration::from_secs(4);
const RETRY: LoginRetry = LoginRetry::new(INITIAL, MAX);
const STORE: &str = "fake-store";
const REFUSAL: &str = "OpenBao returned 503 Service Unavailable: fake-sealed";
const RETRYING: &str = "OpenBao refused the AppRole login; retrying";

type Outcome = Result<&'static str, OpenBaoLoginError>;

fn refused() -> Outcome {
    Err(OpenBaoLoginError::Refused {
        message: REFUSAL.to_owned(),
    })
}

/// Answers each login attempt with the next scripted outcome, and records when
/// it was asked.
struct ScriptedLogin {
    outcomes: RefCell<VecDeque<Outcome>>,
    asked_at: RefCell<Vec<Instant>>,
}

impl ScriptedLogin {
    fn answering(outcomes: impl IntoIterator<Item = Outcome>) -> Self {
        Self {
            outcomes: RefCell::new(outcomes.into_iter().collect()),
            asked_at: RefCell::new(Vec::new()),
        }
    }

    fn attempt(&self) -> Ready<Outcome> {
        self.asked_at.borrow_mut().push(Instant::now());
        ready(
            self.outcomes
                .borrow_mut()
                .pop_front()
                .expect("the login was attempted more often than scripted"),
        )
    }

    fn attempts(&self) -> usize {
        self.asked_at.borrow().len()
    }

    /// How long the retry waited between each attempt and the next.
    fn waits(&self) -> Vec<Duration> {
        self.asked_at
            .borrow()
            .windows(2)
            .map(|pair| pair[1].duration_since(pair[0]))
            .collect()
    }
}

fn retries(events: &CapturedEvents) -> Vec<CapturedEvent> {
    events
        .events()
        .into_iter()
        .filter(|event| event.message() == Some(RETRYING))
        .collect()
}

#[tokio::test(start_paused = true)]
async fn a_login_openbao_cannot_be_configured_for_refuses_at_once_without_a_retry() {
    let login = ScriptedLogin::answering([Err(OpenBaoLoginError::Config(
        OpenBaoSecretStoreConfigError::PlaintextAddress,
    ))]);
    let started = Instant::now();

    let outcome = RETRY.log_in(&CancellationToken::new(), || login.attempt()).await;

    assert!(
        matches!(
            outcome,
            Err(OpenBaoLoginError::Config(
                OpenBaoSecretStoreConfigError::PlaintextAddress
            ))
        ),
        "{outcome:?}"
    );
    assert_eq!(login.attempts(), 1);
    assert_eq!(started.elapsed(), Duration::ZERO);
}

#[tokio::test(start_paused = true)]
async fn a_shutdown_during_the_wait_between_attempts_stops_without_another_attempt() {
    let login = ScriptedLogin::answering([refused()]);
    let shutdown = CancellationToken::new();
    let started = Instant::now();

    let (outcome, ()) = tokio::join!(RETRY.log_in(&shutdown, || login.attempt()), async {
        tokio::time::sleep(INITIAL / 2).await;
        shutdown.cancel();
    });

    assert!(matches!(outcome, Ok(None)), "{outcome:?}");
    assert_eq!(login.attempts(), 1);
    assert_eq!(started.elapsed(), INITIAL / 2);
}

#[tokio::test(start_paused = true)]
async fn a_shutdown_while_openbao_has_not_answered_stops_without_a_store() {
    let attempts = Cell::new(0_usize);
    let shutdown = CancellationToken::new();

    let (outcome, ()) = tokio::join!(
        RETRY.log_in(&shutdown, || {
            attempts.set(attempts.get() + 1);
            pending::<Outcome>()
        }),
        async {
            tokio::time::sleep(MAX).await;
            shutdown.cancel();
        }
    );

    assert!(matches!(outcome, Ok(None)), "{outcome:?}");
    assert_eq!(attempts.get(), 1);
}

#[tokio::test(start_paused = true)]
async fn a_service_told_to_stop_before_it_logs_in_never_asks_openbao() {
    let login = ScriptedLogin::answering([]);
    let shutdown = CancellationToken::new();
    shutdown.cancel();

    let outcome = RETRY.log_in(&shutdown, || login.attempt()).await;

    assert!(matches!(outcome, Ok(None)), "{outcome:?}");
    assert_eq!(login.attempts(), 0);
}

#[tokio::test(start_paused = true)]
async fn a_refused_login_is_retried_after_the_initial_delay_and_yields_the_store_once() {
    let login = ScriptedLogin::answering([refused(), Ok(STORE)]);

    let outcome = RETRY.log_in(&CancellationToken::new(), || login.attempt()).await;

    assert_eq!(outcome.unwrap(), Some(STORE));
    assert_eq!(login.attempts(), 2);
    assert_eq!(login.waits(), [INITIAL]);
}

#[tokio::test(start_paused = true)]
async fn the_delay_between_attempts_doubles_up_to_its_cap() {
    let login = ScriptedLogin::answering([refused(), refused(), refused(), refused(), refused(), Ok(STORE)]);

    let outcome = RETRY.log_in(&CancellationToken::new(), || login.attempt()).await;

    assert_eq!(outcome.unwrap(), Some(STORE));
    assert_eq!(
        login.waits(),
        [INITIAL, INITIAL * 2, MAX, MAX, MAX],
        "the delay doubles from {INITIAL:?} and never passes {MAX:?}"
    );
}

#[tokio::test(start_paused = true)]
async fn each_refused_attempt_is_logged_with_its_number_and_the_delay_before_the_next() {
    let events = CapturedEvents::new();
    let _capture = events.install(LevelFilter::WARN);
    let login = ScriptedLogin::answering([refused(), refused(), Ok(STORE)]);

    RETRY
        .log_in(&CancellationToken::new(), || login.attempt())
        .await
        .unwrap();

    let retries = retries(&events);
    assert_eq!(retries.len(), 2);
    for (retry, (attempt, retry_in)) in retries.iter().zip([("1", "1s"), ("2", "2s")]) {
        assert_eq!(retry.field("attempt"), Some(attempt));
        assert_eq!(retry.field("retry_in"), Some(retry_in));
        assert_eq!(
            retry.field("error"),
            Some(format!("OpenBao refused the AppRole login: {REFUSAL}").as_str())
        );
    }
}

#[tokio::test(start_paused = true)]
async fn a_login_openbao_accepts_at_once_is_neither_retried_nor_logged() {
    let events = CapturedEvents::new();
    let _capture = events.install(LevelFilter::WARN);
    let login = ScriptedLogin::answering([Ok(STORE)]);
    let started = Instant::now();

    let outcome = RETRY.log_in(&CancellationToken::new(), || login.attempt()).await;

    assert_eq!(outcome.unwrap(), Some(STORE));
    assert_eq!(login.attempts(), 1);
    assert_eq!(started.elapsed(), Duration::ZERO);
    assert!(retries(&events).is_empty());
}
