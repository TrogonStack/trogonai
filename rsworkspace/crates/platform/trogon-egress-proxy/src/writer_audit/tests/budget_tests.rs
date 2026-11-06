use super::*;
use crate::connection_limit::ConnectionLimit;

fn with_limit(sink: MemorySink, limit: ConnectionLimit) -> WriterAudit<MemorySink> {
    let identity = WriterIdentity::new(
        writer(),
        NatsToken::new("egress_proxy").expect("component"),
        AuditName::new("acme").expect("tenant"),
        NatsToken::new("payments").expect("project"),
    );
    let stamper = Stamper::new(SystemClock, UuidV7Generator);
    let writer = AuditWriter::new(sink, identity.clone(), WriterConfig::default(), &stamper);
    WriterAudit::with_limit(writer, identity.tenant().clone(), identity.project().clone(), limit)
}

#[tokio::test]
async fn dropped_callers_cannot_refill_the_actual_audit_worker_budget() {
    let sink = MemorySink::new();
    sink.hold();
    let limit = ConnectionLimit::new(2).expect("bounded limit");
    let audit = with_limit(sink.clone(), limit);
    let agent = AgentId::new("agent-1").expect("agent");
    let destination = "api.example.com:443".parse().expect("destination");
    let policy = source();
    let request = event(&agent, Some(&destination), &policy, Decision::Permitted);
    assert_eq!(audit.capacity().limit(), limit);
    assert!(audit.capacity().accepting());
    for index in 0..limit.get() {
        let mut caller = Box::pin(audit.record(&request));
        assert!(futures::poll!(caller.as_mut()).is_pending());
        drop(caller);
        assert_eq!(audit.capacity().in_flight(), index + 1);
    }
    assert!(!audit.capacity().has_capacity());
    let captured = CapturedEvents::new();
    let _guard = captured.install(LevelFilter::INFO);
    let excess = tokio::time::timeout(std::time::Duration::from_millis(50), audit.record(&request)).await;
    assert_eq!(audit.capacity().in_flight(), limit.get());
    sink.release();
    let shutdown = CancellationToken::new();
    shutdown.cancel();
    audit.run(shutdown).await;
    assert!(
        matches!(excess, Ok(Err(AuditError::Overloaded))),
        "excess request must be refused immediately: {excess:?}"
    );
    assert_eq!(decisions(&sink).len(), limit.get());
    assert_eq!(audit.capacity().in_flight(), 0);
    assert!(!audit.capacity().accepting());
    assert!(!audit.capacity().has_capacity());
    let lines: Vec<_> = captured
        .events()
        .into_iter()
        .filter(|line| line.message() == Some("egress decision"))
        .collect();
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].field("audit_failure"), Some("overloaded"));
    assert_eq!(lines[0].field("fault"), Some("AUDIT_UNAVAILABLE"));
}

#[tokio::test]
async fn completed_detached_writes_restore_admission_without_replacing_the_budget() {
    let sink = MemorySink::new();
    sink.hold();
    let audit = with_limit(sink.clone(), ConnectionLimit::new(1).expect("limit"));
    let agent = AgentId::new("agent-1").expect("agent");
    let destination = "api.example.com:443".parse().expect("destination");
    let policy = source();
    let request = event(&agent, Some(&destination), &policy, Decision::Permitted);
    let mut caller = Box::pin(audit.record(&request));
    assert!(futures::poll!(caller.as_mut()).is_pending());
    drop(caller);
    assert_eq!(audit.capacity().in_flight(), 1);
    sink.release();
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while audit.capacity().in_flight() != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the actual write settles");
    assert!(audit.capacity().has_capacity());
    audit
        .record(&request)
        .await
        .expect("new admission after terminal write");
    assert_eq!(decisions(&sink).len(), 2);
    let shutdown = CancellationToken::new();
    shutdown.cancel();
    audit.run(shutdown).await;
}

#[tokio::test]
async fn shutdown_refuses_new_writes_and_waits_for_a_cancelled_callers_actual_write() {
    let sink = MemorySink::new();
    sink.hold();
    let audit = with_limit(sink.clone(), ConnectionLimit::new(1).expect("limit"));
    let agent = AgentId::new("agent-1").expect("agent");
    let destination = "api.example.com:443".parse().expect("destination");
    let policy = source();
    let request = event(&agent, Some(&destination), &policy, Decision::Refused(Fault::NoPermit));
    let mut caller = Box::pin(audit.record(&request));
    assert!(futures::poll!(caller.as_mut()).is_pending());
    drop(caller);
    let shutdown = CancellationToken::new();
    shutdown.cancel();
    let mut run = Box::pin(audit.run(shutdown));
    assert!(futures::poll!(run.as_mut()).is_pending());
    assert!(!audit.capacity().accepting());
    assert_eq!(audit.record(&request).await, Err(AuditError::Overloaded));
    assert_eq!(audit.capacity().in_flight(), 1);
    sink.release();
    run.await;
    assert_eq!(audit.capacity().in_flight(), 0);
    assert_eq!(decisions(&sink).len(), 1);
}

#[tokio::test]
async fn record_validation_returns_its_slot_before_any_worker_is_spawned() {
    let sink = MemorySink::new();
    let audit = with_limit(sink.clone(), ConnectionLimit::new(1).expect("limit"));
    let agent = AgentId::new("agent-1").expect("agent");
    let destination = "api.example.com:443".parse().expect("destination");
    let policy = source();
    let mut request = event(&agent, Some(&destination), &policy, Decision::Permitted);
    request.policy_revision = Some(PolicyRevision {
        incarnation: NonZeroU64::MIN,
        generation: NonZeroU64::MIN,
    });
    assert_eq!(audit.record(&request).await, Err(AuditError::PolicyEvidence));
    assert_eq!(audit.capacity().in_flight(), 0);
    assert_eq!(sink.attempts(), 0);
    request.policy_revision = None;
    audit
        .record(&request)
        .await
        .expect("slot available after validation refusal");
    assert_eq!(decisions(&sink).len(), 1);
}

#[test]
fn existing_constructor_has_finite_default_capacity() {
    let (audit, _) = audit();
    assert_eq!(audit.capacity().limit(), ConnectionLimit::default());
    assert_eq!(audit.capacity().in_flight(), 0);
    assert!(audit.capacity().has_capacity());
}
