use super::*;
use crate::audit_readiness::AuditReadiness;

#[derive(Clone)]
struct PanickingSink;

impl Sink for PanickingSink {
    async fn publish(&self, _: &SealedRecord) -> Result<StoredAck, SinkError> {
        panic!("fixture audit storage panic");
    }
}

#[tokio::test]
async fn a_panicking_storage_worker_cannot_make_readiness_healthy_or_leak_capacity() {
    let audit = audit_over(PanickingSink);
    let events = CapturedEvents::new();
    let _capture = events.install(LevelFilter::WARN);
    assert_eq!(audit.readiness().await, AuditReadiness::Unavailable);
    assert_eq!(audit.capacity().in_flight(), 0);
    assert!(
        events
            .events()
            .iter()
            .any(|event| event.message() == Some("audit readiness worker ended abnormally"))
    );
}

#[tokio::test]
async fn cancelled_and_timed_out_readiness_probes_keep_one_worker_until_actual_completion() {
    let (audit, sink) = audit();
    sink.hold();
    let mut probe = Box::pin(audit.readiness());
    assert!(futures::poll!(probe.as_mut()).is_pending());
    drop(probe);
    assert_eq!(audit.capacity().in_flight(), 1);
    for _ in 0..audit.capacity().limit().get() {
        assert_eq!(audit.readiness().await, AuditReadiness::Overloaded);
        assert_eq!(audit.capacity().in_flight(), 1);
    }
    sink.release();
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while audit.capacity().in_flight() != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the real cancelled probe settles");
    sink.hold();
    assert_eq!(audit.readiness().await, AuditReadiness::TimedOut);
    assert_eq!(audit.capacity().in_flight(), 1);
    assert_eq!(audit.readiness().await, AuditReadiness::Overloaded);
    sink.release();
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while audit.capacity().in_flight() != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the real timed-out probe settles");
    assert_eq!(audit.readiness().await, AuditReadiness::Ready);
    let shutdown = CancellationToken::new();
    shutdown.cancel();
    audit.run(shutdown).await;
    assert_eq!(audit.readiness().await, AuditReadiness::Degraded);
    assert_eq!(audit.capacity().in_flight(), 0);
}

#[tokio::test]
async fn a_stopped_audit_stays_degraded_without_probing_recovered_storage() {
    let (audit, sink) = audit();
    assert_eq!(audit.readiness().await, AuditReadiness::Ready);
    let shutdown = CancellationToken::new();
    shutdown.cancel();
    audit.run(shutdown.clone()).await;
    let attempts = sink.attempts();
    let stored = sink.stored().len();

    assert_eq!(audit.readiness().await, AuditReadiness::Degraded);
    sink.fail_all(SinkError::Unreachable);
    assert_eq!(audit.readiness().await, AuditReadiness::Degraded);
    sink.recover();
    assert_eq!(audit.readiness().await, AuditReadiness::Degraded);
    let (outcome, _) = recorded(&audit, Some("api.example.com:443"), Decision::Permitted).await;
    assert_eq!(outcome, Err(AuditError::Overloaded));
    audit.run(shutdown).await;

    assert_eq!(sink.attempts(), attempts);
    assert_eq!(sink.stored().len(), stored);
    assert_eq!(audit.capacity().in_flight(), 0);
    assert!(!audit.capacity().has_capacity());
}

#[tokio::test]
async fn readiness_cannot_consume_the_last_decision_worker_slot() {
    let (audit, sink) = audit();
    sink.hold();
    let agent = AgentId::new("agent-1").expect("agent");
    let destination = "api.example.com:443".parse().expect("destination");
    let policy = source();
    let request = event(&agent, Some(&destination), &policy, Decision::Permitted);
    let limit = audit.capacity().limit().get();
    for _ in 1..limit {
        let mut caller = Box::pin(audit.record(&request));
        assert!(futures::poll!(caller.as_mut()).is_pending());
        drop(caller);
    }
    assert_eq!(audit.capacity().in_flight(), limit - 1);
    assert_eq!(audit.readiness().await, AuditReadiness::Overloaded);
    assert_eq!(audit.capacity().in_flight(), limit - 1);
    let mut caller = Box::pin(audit.record(&request));
    assert!(futures::poll!(caller.as_mut()).is_pending());
    drop(caller);
    assert_eq!(audit.capacity().in_flight(), limit);
    sink.release();
    let shutdown = CancellationToken::new();
    shutdown.cancel();
    audit.run(shutdown).await;
    assert_eq!(decisions(&sink).len(), limit);
}

#[tokio::test]
async fn readiness_requires_a_fresh_integrity_ack_and_refuses_a_previously_healthy_failed_sink() {
    let (audit, sink) = audit();
    assert_eq!(audit.readiness().await, AuditReadiness::Ready);
    assert_eq!(sink.stored().len(), 2, "genesis and one readiness probe");
    let records = sink.stored_records();
    let genesis = sink.stored().first().expect("genesis").clone();
    let verified = verify_chain_range(
        ChainAnchor::Genesis {
            writer: writer(),
            chain: genesis.chain(),
        },
        records,
    )
    .expect("the readiness probe preserves the hash chain");
    assert_eq!(verified.records(), 2);
    let probe = sink.stored().last().expect("probe").clone();
    assert_eq!(
        probe.subject(),
        "platform.v1.audit.integrity.egress_proxy.readiness_probe"
    );
    sink.fail_all(SinkError::Unreachable);
    assert_eq!(audit.readiness().await, AuditReadiness::Unavailable);
    assert_eq!(audit.readiness().await, AuditReadiness::Degraded);
    sink.recover();
    audit.writer.drain_once().await;
    let before = sink.stored().len();
    assert_eq!(audit.readiness().await, AuditReadiness::Ready);
    assert_eq!(
        sink.stored().len(),
        before + 1,
        "recovery must acknowledge another fresh probe"
    );
}
