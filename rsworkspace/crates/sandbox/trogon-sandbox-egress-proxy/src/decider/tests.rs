use super::*;
use futures::StreamExt;
use std::sync::MutexGuard;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::{Notify, Semaphore};
use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_contract::LeaseError;

type BoxError = Box<dyn std::error::Error + Send + Sync>;
type TestResult = Result<(), BoxError>;

fn lease() -> LeaseId {
    LeaseId::new("lease-1")
}

fn host(name: &str) -> Result<Host, BoxError> {
    Ok(Host::new(name)?)
}

fn port(port: u16) -> Result<Port, BoxError> {
    Ok(Port::new(port)?)
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

enum Script {
    Reply(&'static str),
    Fail(fn() -> DeciderTransportError),
    Hang,
}

struct FakeTransport {
    script: Script,
    requests: Mutex<Vec<(String, DecideRequest)>>,
    started: Notify,
    release: Semaphore,
    held: bool,
}

impl FakeTransport {
    fn new(script: Script) -> Arc<Self> {
        Arc::new(Self {
            script,
            requests: Mutex::new(Vec::new()),
            started: Notify::new(),
            release: Semaphore::new(0),
            held: false,
        })
    }

    fn held(script: Script) -> Arc<Self> {
        Arc::new(Self {
            script,
            requests: Mutex::new(Vec::new()),
            started: Notify::new(),
            release: Semaphore::new(0),
            held: true,
        })
    }

    fn seen(&self) -> Vec<(String, DecideRequest)> {
        locked(&self.requests).clone()
    }
}

#[async_trait]
impl DeciderTransport for FakeTransport {
    async fn request(&self, subject: String, payload: Vec<u8>) -> Result<Vec<u8>, DeciderTransportError> {
        let request =
            serde_json::from_slice(&payload).map_err(|error| DeciderTransportError::Unavailable(error.to_string()))?;
        locked(&self.requests).push((subject, request));
        self.started.notify_waiters();
        if self.held {
            self.release
                .acquire()
                .await
                .map_err(|error| DeciderTransportError::Unavailable(error.to_string()))?
                .forget();
        }
        match &self.script {
            Script::Reply(body) => Ok(body.as_bytes().to_vec()),
            Script::Fail(make) => Err(make()),
            Script::Hang => std::future::pending().await,
        }
    }
}

#[derive(Default)]
struct RecordingAmender {
    calls: Mutex<Vec<(Host, Port, Decision)>>,
}

#[async_trait]
impl PolicyAmender for RecordingAmender {
    async fn amend(&self, host: &Host, port: Port, decision: Decision) {
        locked(&self.calls).push((host.clone(), port, decision));
    }
}

fn decider(transport: Arc<FakeTransport>, amender: Arc<RecordingAmender>, limits: DeciderLimits) -> Decider {
    Decider::new(lease(), transport, amender, limits)
}

async fn asked(script: Script) -> Result<(Judgment, Arc<FakeTransport>, Arc<RecordingAmender>), BoxError> {
    let transport = FakeTransport::new(script);
    let amender = Arc::new(RecordingAmender::default());
    let decider = decider(transport.clone(), amender.clone(), DeciderLimits::default());
    let judgment = decider
        .decide(&host("api.example.com")?, port(443)?, Some(DecideMethod::Connect))
        .await;
    Ok((judgment, transport, amender))
}

#[tokio::test]
async fn the_request_carries_lease_host_port_and_method_on_the_lease_subject() -> TestResult {
    let (judgment, transport, _) = asked(Script::Reply(r#"{"verdict":"allow","scope":"once"}"#)).await?;
    assert_eq!(judgment, Judgment::Allow);
    let seen = transport.seen();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].0, "sandbox.egress.decide.lease-1");
    assert_eq!(
        seen[0].1,
        DecideRequest {
            lease_id: lease(),
            host: host("api.example.com")?,
            port: port(443)?,
            method: Some(DecideMethod::Connect),
        }
    );
    Ok(())
}

#[tokio::test]
async fn a_once_allow_changes_no_policy() -> TestResult {
    let (judgment, _, amender) = asked(Script::Reply(r#"{"verdict":"allow","scope":"once"}"#)).await?;
    assert_eq!(judgment, Judgment::Allow);
    assert!(locked(&amender.calls).is_empty());
    Ok(())
}

#[tokio::test]
async fn a_persisted_allow_amends_the_policy() -> TestResult {
    let (judgment, _, amender) = asked(Script::Reply(r#"{"verdict":"allow","scope":"persist"}"#)).await?;
    assert_eq!(judgment, Judgment::Allow);
    assert_eq!(
        *locked(&amender.calls),
        vec![(host("api.example.com")?, port(443)?, Decision::Allow)]
    );
    Ok(())
}

#[tokio::test]
async fn a_deny_is_reported_and_can_be_persisted() -> TestResult {
    let (judgment, _, amender) = asked(Script::Reply(r#"{"verdict":"deny","scope":"once"}"#)).await?;
    assert_eq!(judgment, Judgment::Deny(EgressViolationReason::DeciderDenied));
    assert!(locked(&amender.calls).is_empty());

    let (judgment, _, amender) = asked(Script::Reply(r#"{"verdict":"deny","scope":"persist"}"#)).await?;
    assert_eq!(judgment, Judgment::Deny(EgressViolationReason::DeciderDenied));
    assert_eq!(
        *locked(&amender.calls),
        vec![(host("api.example.com")?, port(443)?, Decision::Deny)]
    );
    Ok(())
}

#[tokio::test]
async fn malformed_and_unknown_replies_deny_without_amending() -> TestResult {
    let oversized: &'static str = Box::leak(
        format!(
            r#"{{"verdict":"allow","scope":"once","pad":"{}"}}"#,
            "x".repeat(MAX_REPLY_BYTES)
        )
        .into_boxed_str(),
    );
    for body in [
        "",
        "not json",
        "{}",
        r#"{"verdict":"maybe","scope":"once"}"#,
        r#"{"verdict":"allow"}"#,
        r#"{"verdict":"allow","scope":"forever"}"#,
        r#"{"verdict":"allow","scope":"persist","extra":true}"#,
        oversized,
    ] {
        let (judgment, _, amender) = asked(Script::Reply(body)).await?;
        assert_eq!(
            judgment,
            Judgment::Deny(EgressViolationReason::DeciderMalformedReply),
            "{body}"
        );
        assert!(locked(&amender.calls).is_empty());
    }
    Ok(())
}

#[tokio::test]
async fn an_unreachable_decider_denies() -> TestResult {
    let (judgment, _, _) = asked(Script::Fail(|| {
        DeciderTransportError::Unavailable("no responders".to_string())
    }))
    .await?;
    assert_eq!(judgment, Judgment::Deny(EgressViolationReason::DeciderUnavailable));
    Ok(())
}

#[tokio::test]
async fn a_transport_timeout_denies_as_timed_out() -> TestResult {
    let (judgment, _, _) = asked(Script::Fail(|| DeciderTransportError::TimedOut)).await?;
    assert_eq!(judgment, Judgment::Deny(EgressViolationReason::DeciderTimedOut));
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_silent_decider_times_out_and_denies() -> TestResult {
    let transport = FakeTransport::new(Script::Hang);
    let amender = Arc::new(RecordingAmender::default());
    let limits = DeciderLimits {
        timeout: Duration::from_secs(2),
        max_in_flight: 4,
    };
    let decider = decider(transport, amender.clone(), limits);
    let judgment = decider.decide(&host("api.example.com")?, port(443)?, None).await;
    assert_eq!(judgment, Judgment::Deny(EgressViolationReason::DeciderTimedOut));
    assert!(locked(&amender.calls).is_empty());
    Ok(())
}

#[tokio::test]
async fn concurrent_asks_for_one_destination_send_one_request_and_share_it() -> TestResult {
    let transport = FakeTransport::held(Script::Reply(r#"{"verdict":"allow","scope":"once"}"#));
    let amender = Arc::new(RecordingAmender::default());
    let decider = Arc::new(decider(transport.clone(), amender, DeciderLimits::default()));

    let started = transport.started.notified();
    tokio::pin!(started);
    started.as_mut().enable();
    let mut asks = Vec::new();
    for _ in 0..5 {
        let decider = Arc::clone(&decider);
        asks.push(tokio::spawn(async move {
            Ok::<Judgment, BoxError>(decider.decide(&host("api.example.com")?, port(443)?, None).await)
        }));
    }
    started.await;
    tokio::task::yield_now().await;
    transport.release.add_permits(100);

    for ask in asks {
        assert_eq!(ask.await??, Judgment::Allow);
    }
    assert_eq!(transport.seen().len(), 1);
    Ok(())
}

#[tokio::test]
async fn a_settled_question_is_asked_again_next_time() -> TestResult {
    let transport = FakeTransport::new(Script::Reply(r#"{"verdict":"allow","scope":"once"}"#));
    let decider = decider(
        transport.clone(),
        Arc::new(RecordingAmender::default()),
        DeciderLimits::default(),
    );
    for _ in 0..2 {
        decider.decide(&host("api.example.com")?, port(443)?, None).await;
    }
    assert_eq!(transport.seen().len(), 2);
    Ok(())
}

#[tokio::test]
async fn another_port_is_another_question() -> TestResult {
    let transport = FakeTransport::new(Script::Reply(r#"{"verdict":"allow","scope":"once"}"#));
    let decider = decider(
        transport.clone(),
        Arc::new(RecordingAmender::default()),
        DeciderLimits::default(),
    );
    decider.decide(&host("api.example.com")?, port(443)?, None).await;
    decider.decide(&host("api.example.com")?, port(8443)?, None).await;
    assert_eq!(transport.seen().len(), 2);
    Ok(())
}

#[tokio::test]
async fn excess_distinct_questions_are_denied_without_being_asked() -> TestResult {
    let transport = FakeTransport::held(Script::Reply(r#"{"verdict":"allow","scope":"once"}"#));
    let limits = DeciderLimits {
        timeout: Duration::from_secs(30),
        max_in_flight: 2,
    };
    let decider = Arc::new(decider(
        transport.clone(),
        Arc::new(RecordingAmender::default()),
        limits,
    ));

    let mut pending = Vec::new();
    for name in ["a.example.com", "b.example.com"] {
        let decider = Arc::clone(&decider);
        pending.push(tokio::spawn(async move {
            Ok::<Judgment, BoxError>(decider.decide(&host(name)?, port(443)?, None).await)
        }));
    }
    while transport.seen().len() < 2 {
        tokio::task::yield_now().await;
    }

    let over = decider.decide(&host("c.example.com")?, port(443)?, None).await;
    assert_eq!(over, Judgment::Deny(EgressViolationReason::DeciderOverloaded));
    assert_eq!(transport.seen().len(), 2);

    transport.release.add_permits(100);
    for task in pending {
        assert_eq!(task.await??, Judgment::Allow);
    }
    assert_eq!(
        decider.decide(&host("c.example.com")?, port(443)?, None).await,
        Judgment::Allow
    );
    Ok(())
}

#[tokio::test]
async fn a_caller_that_gives_up_does_not_strand_the_others() -> TestResult {
    let transport = FakeTransport::held(Script::Reply(r#"{"verdict":"allow","scope":"once"}"#));
    let decider = Arc::new(decider(
        transport.clone(),
        Arc::new(RecordingAmender::default()),
        DeciderLimits::default(),
    ));

    let first = {
        let decider = Arc::clone(&decider);
        tokio::spawn(async move {
            Ok::<Judgment, BoxError>(decider.decide(&host("api.example.com")?, port(443)?, None).await)
        })
    };
    while transport.seen().is_empty() {
        tokio::task::yield_now().await;
    }
    let second = {
        let decider = Arc::clone(&decider);
        tokio::spawn(async move {
            Ok::<Judgment, BoxError>(decider.decide(&host("api.example.com")?, port(443)?, None).await)
        })
    };
    tokio::task::yield_now().await;
    first.abort();
    tokio::task::yield_now().await;
    transport.release.add_permits(100);

    assert_eq!(second.await??, Judgment::Allow);
    assert_eq!(transport.seen().len(), 1);
    Ok(())
}

#[tokio::test]
async fn an_unlisted_destination_with_no_decider_is_a_policy_deny() -> TestResult {
    let vetted = Ok(VettedAddr::assume_vetted("203.0.113.5".parse()?));
    assert_eq!(
        Decider::decide_unlisted(None, &vetted, Some("api.example.com"), "203.0.113.5".parse()?, 443).await,
        Err(EgressViolationReason::PolicyDenied)
    );
    Ok(())
}

#[tokio::test]
async fn the_ssrf_floor_is_settled_before_the_decider_is_asked() -> TestResult {
    let transport = FakeTransport::new(Script::Reply(r#"{"verdict":"allow","scope":"persist"}"#));
    let amender = Arc::new(RecordingAmender::default());
    let decider = decider(transport.clone(), amender.clone(), DeciderLimits::default());
    let vetted: Result<VettedAddr, _> = Err(AddressDenialClass::CloudMetadata);
    assert_eq!(
        Decider::decide_unlisted(
            Some(&decider),
            &vetted,
            Some("metadata.example.com"),
            "169.254.169.254".parse()?,
            80
        )
        .await,
        Err(EgressViolationReason::DestinationAddressDenied(
            AddressDenialClass::CloudMetadata
        ))
    );
    assert!(transport.seen().is_empty());
    assert!(locked(&amender.calls).is_empty());
    Ok(())
}

#[tokio::test]
async fn transparent_asks_name_the_sni_as_the_method_and_fall_back_to_the_address() -> TestResult {
    let transport = FakeTransport::new(Script::Reply(r#"{"verdict":"allow","scope":"once"}"#));
    let decider = decider(
        transport.clone(),
        Arc::new(RecordingAmender::default()),
        DeciderLimits::default(),
    );
    let vetted = Ok(VettedAddr::assume_vetted("203.0.113.5".parse()?));
    let ip = "203.0.113.5".parse()?;
    assert_eq!(
        Decider::decide_unlisted(Some(&decider), &vetted, Some("api.example.com"), ip, 443).await,
        Ok(())
    );
    assert_eq!(
        Decider::decide_unlisted(Some(&decider), &vetted, None, ip, 443).await,
        Ok(())
    );
    let seen = transport.seen();
    assert_eq!(seen[0].1.host, host("api.example.com")?);
    assert_eq!(seen[0].1.method, Some(DecideMethod::TlsSni));
    assert_eq!(seen[1].1.host, Host::from(ip));
    assert_eq!(seen[1].1.method, None);
    Ok(())
}

struct RecordingPublisher {
    published: Mutex<Vec<NetworkPolicy>>,
    calls: AtomicUsize,
    done: Notify,
}

#[async_trait]
impl PolicyPublisher for RecordingPublisher {
    async fn publish(&self, lease: &LeaseId, policy: &NetworkPolicy) -> Result<(), String> {
        assert_eq!(lease, &LeaseId::new("lease-1"));
        locked(&self.published).push(policy.clone());
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.done.notify_one();
        Ok(())
    }
}

fn user_defined() -> NetworkPolicy {
    NetworkPolicy::UserDefined {
        rules: Vec::new(),
        allow_ranges: Vec::new(),
        deny_ranges: Vec::new(),
        denied_domains: Vec::new(),
        opaque_tunnels: trogon_sandbox_contract::OpaqueTunnelPolicy::Allow,
        request_rules: vec![],
    }
}

#[tokio::test]
async fn the_live_amender_updates_the_served_policy_and_publishes_it() -> TestResult {
    let live = Arc::new(RwLock::new(user_defined()));
    let publisher = Arc::new(RecordingPublisher {
        published: Mutex::new(Vec::new()),
        calls: AtomicUsize::new(0),
        done: Notify::new(),
    });
    let amender = LivePolicyAmender::new(lease(), live.clone(), publisher.clone());

    amender
        .amend(&host("api.example.com")?, port(443)?, Decision::Allow)
        .await;

    let served = live.read().await.clone();
    assert_eq!(
        crate::policy::rule_host(&served, &host("api.example.com")?, 443),
        crate::policy::Ruling::Allow(Some(trogon_sandbox_contract::DomainRuleAction::PassThrough))
    );
    publisher.done.notified().await;
    assert_eq!(locked(&publisher.published).as_slice(), [served]);
    Ok(())
}

#[tokio::test]
async fn the_live_amender_skips_a_verdict_the_policy_already_holds() -> TestResult {
    let live = Arc::new(RwLock::new(user_defined()));
    let publisher = Arc::new(RecordingPublisher {
        published: Mutex::new(Vec::new()),
        calls: AtomicUsize::new(0),
        done: Notify::new(),
    });
    let amender = LivePolicyAmender::new(lease(), live.clone(), publisher.clone());

    amender
        .amend(&host("api.example.com")?, port(443)?, Decision::Deny)
        .await;
    publisher.done.notified().await;
    amender
        .amend(&host("api.example.com")?, port(443)?, Decision::Deny)
        .await;
    tokio::task::yield_now().await;

    assert_eq!(publisher.calls.load(Ordering::SeqCst), 1);
    Ok(())
}

#[tokio::test]
async fn the_live_amender_leaves_a_fixed_policy_alone() -> TestResult {
    let live = Arc::new(RwLock::new(NetworkPolicy::DenyAll));
    let publisher = Arc::new(RecordingPublisher {
        published: Mutex::new(Vec::new()),
        calls: AtomicUsize::new(0),
        done: Notify::new(),
    });
    let amender = LivePolicyAmender::new(lease(), live.clone(), publisher.clone());

    amender
        .amend(&host("api.example.com")?, port(443)?, Decision::Allow)
        .await;
    tokio::task::yield_now().await;

    assert_eq!(*live.read().await, NetworkPolicy::DenyAll);
    assert_eq!(publisher.calls.load(Ordering::SeqCst), 0);
    Ok(())
}

struct FailingPublisher {
    entered: Notify,
}

#[async_trait]
impl PolicyPublisher for FailingPublisher {
    async fn publish(&self, _lease: &LeaseId, _policy: &NetworkPolicy) -> Result<(), String> {
        self.entered.notify_one();
        Err("the broker is gone".to_string())
    }
}

#[tokio::test]
async fn the_live_amender_survives_a_publisher_that_fails() -> TestResult {
    let live = Arc::new(RwLock::new(user_defined()));
    let publisher = Arc::new(FailingPublisher { entered: Notify::new() });
    let amender = LivePolicyAmender::new(lease(), live.clone(), publisher.clone());

    amender
        .amend(&host("api.example.com")?, port(443)?, Decision::Allow)
        .await;
    publisher.entered.notified().await;
    let _finished = amender.publishing.lock().await;

    assert_ne!(*live.read().await, user_defined());
    Ok(())
}

#[tokio::test]
async fn an_unsettled_question_releases_its_waiters_with_a_deny_when_dropped() -> TestResult {
    let decider = decider(
        FakeTransport::new(Script::Hang),
        Arc::new(RecordingAmender::default()),
        DeciderLimits::default(),
    );
    let question = Question {
        host: host("api.example.com")?,
        port: port(443)?,
    };
    let (reply, mut answer) = oneshot::channel();
    decider.shared.pending().insert(question.clone(), vec![reply]);
    let settlement = Settlement {
        shared: Arc::clone(&decider.shared),
        question,
        settled: false,
    };

    drop(settlement);

    assert_eq!(
        answer.try_recv()?,
        Judgment::Deny(EgressViolationReason::DeciderUnavailable)
    );
    assert!(decider.shared.pending().is_empty());
    Ok(())
}

#[tokio::test]
async fn a_transparent_ask_with_an_unusable_name_or_port_is_a_policy_deny() -> TestResult {
    let transport = FakeTransport::new(Script::Reply(r#"{"verdict":"allow","scope":"once"}"#));
    let decider = decider(
        transport.clone(),
        Arc::new(RecordingAmender::default()),
        DeciderLimits::default(),
    );
    let vetted = Ok(VettedAddr::assume_vetted("203.0.113.5".parse()?));
    let ip = "203.0.113.5".parse()?;
    assert_eq!(
        Decider::decide_unlisted(Some(&decider), &vetted, Some(""), ip, 443).await,
        Err(EgressViolationReason::PolicyDenied)
    );
    assert_eq!(
        Decider::decide_unlisted(Some(&decider), &vetted, Some("api.example.com"), ip, 0).await,
        Err(EgressViolationReason::PolicyDenied)
    );
    assert!(transport.seen().is_empty());
    Ok(())
}

#[tokio::test]
async fn a_transparent_ask_the_decider_refuses_reports_the_reason() -> TestResult {
    let transport = FakeTransport::new(Script::Reply(r#"{"verdict":"deny","scope":"once"}"#));
    let decider = decider(
        transport,
        Arc::new(RecordingAmender::default()),
        DeciderLimits::default(),
    );
    let vetted = Ok(VettedAddr::assume_vetted("203.0.113.5".parse()?));
    assert_eq!(
        Decider::decide_unlisted(
            Some(&decider),
            &vetted,
            Some("api.example.com"),
            "203.0.113.5".parse()?,
            443
        )
        .await,
        Err(EgressViolationReason::DeciderDenied)
    );
    Ok(())
}

async fn reply_on(client: &async_nats::Client, subject: &str, body: Option<Vec<u8>>) -> TestResult {
    let mut subscriber = client.subscribe(subject.to_string()).await?;
    client.flush().await?;
    let responder = client.clone();
    tokio::spawn(async move {
        while let Some(message) = subscriber.next().await {
            if let (Some(body), Some(reply)) = (&body, message.reply) {
                let _ = responder.publish(reply, body.clone().into()).await;
            }
        }
    });
    Ok(())
}

async fn client_with_request_timeout(
    server: &JetStreamTestServer,
    request_timeout: Duration,
) -> Result<async_nats::Client, BoxError> {
    Ok(async_nats::ConnectOptions::new()
        .request_timeout(Some(request_timeout))
        .connect(server.address())
        .await?)
}

#[tokio::test]
async fn the_nats_transport_relays_replies_and_classifies_failures() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let client = client_with_request_timeout(&server, Duration::from_millis(500)).await?;
    let transport = NatsDeciderTransport::new(client.clone());

    reply_on(&client, "decide.answered", Some(b"yes".to_vec())).await?;
    assert_eq!(
        transport.request("decide.answered".to_string(), b"?".to_vec()).await?,
        b"yes".to_vec()
    );

    reply_on(&client, "decide.silent", None).await?;
    assert!(matches!(
        transport.request("decide.silent".to_string(), b"?".to_vec()).await,
        Err(DeciderTransportError::TimedOut)
    ));

    assert!(matches!(
        transport.request("decide.nobody".to_string(), b"?".to_vec()).await,
        Err(DeciderTransportError::Unavailable(_))
    ));
    Ok(())
}

#[tokio::test]
async fn the_nats_policy_publisher_reports_each_way_the_broker_can_answer() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let client = server.client().await;
    let mut publisher = NatsPolicyPublisher::new(client.clone());
    publisher.timeout = Duration::from_millis(500);
    let policy = user_defined();
    let subject = lease_network_policy_update(&lease());

    assert!(publisher.publish(&lease(), &policy).await.is_err());

    reply_on(&client, &subject, None).await?;
    assert!(publisher.publish(&lease(), &policy).await.is_err());

    let applied = serde_json::to_vec(&NetworkPolicyUpdateResponse::Applied)?;
    let other = lease_network_policy_update(&LeaseId::new("lease-applied"));
    reply_on(&client, &other, Some(applied)).await?;
    assert_eq!(publisher.publish(&LeaseId::new("lease-applied"), &policy).await, Ok(()));

    let denied = serde_json::to_vec(&NetworkPolicyUpdateResponse::Denied(LeaseError::NoProviderAvailable))?;
    let denying = lease_network_policy_update(&LeaseId::new("lease-denied"));
    reply_on(&client, &denying, Some(denied)).await?;
    assert!(publisher.publish(&LeaseId::new("lease-denied"), &policy).await.is_err());

    let garbled = lease_network_policy_update(&LeaseId::new("lease-garbled"));
    reply_on(&client, &garbled, Some(b"not json".to_vec())).await?;
    assert!(
        publisher
            .publish(&LeaseId::new("lease-garbled"), &policy)
            .await
            .is_err()
    );
    Ok(())
}
