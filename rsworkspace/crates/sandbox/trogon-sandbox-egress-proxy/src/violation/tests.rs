use super::*;
use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_contract::EgressViolationReason;

#[derive(Default)]
struct CollectingSink {
    events: Mutex<Vec<(String, Vec<u8>)>>,
}

impl CollectingSink {
    fn taken(&self) -> Vec<(String, Vec<u8>)> {
        self.events
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

#[async_trait]
impl ViolationSink for CollectingSink {
    async fn publish(&self, subject: String, payload: Vec<u8>) -> Result<(), ViolationPublishError> {
        self.events
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push((subject, payload));
        Ok(())
    }
}

fn lease(id: &str) -> LeaseId {
    LeaseId::new(id)
}

fn dest() -> Result<SocketAddr, std::net::AddrParseError> {
    "203.0.113.7:443".parse()
}

fn denial() -> Denial {
    Denial::new(
        Some("evil.example.com".to_string()),
        EgressViolationReason::PolicyDenied,
    )
}

#[tokio::test]
async fn publishes_one_event_per_denial_on_lease_subject() -> Result<(), Box<dyn std::error::Error>> {
    let sink = Arc::new(CollectingSink::default());
    let reporter = ViolationReporter::new(lease("L1"), sink.clone(), ViolationRateLimit::default());

    for _ in 0..3 {
        reporter.report_outcome(&ProxyOutcome::Denied(denial()), dest()?).await;
    }

    let events = sink.taken();
    assert_eq!(events.len(), 3);
    assert!(events.iter().all(|(s, _)| s == "sandbox.egress.violation.L1"));
    let parsed: EgressViolation = serde_json::from_slice(&events[0].1)?;
    assert_eq!(parsed.lease_id, lease("L1"));
    assert_eq!(parsed.host.as_deref(), Some("evil.example.com"));
    assert_eq!(parsed.port, 443);
    assert_eq!(parsed.dest_ip, Some(dest()?.ip()));
    assert_eq!(parsed.decision, EgressDecision::Deny);
    assert_eq!(parsed.reason, EgressViolationReason::PolicyDenied);
    Ok(())
}

#[tokio::test]
async fn floor_denials_carry_their_class_in_the_event() -> Result<(), Box<dyn std::error::Error>> {
    let sink = Arc::new(CollectingSink::default());
    let reporter = ViolationReporter::new(lease("L1"), sink.clone(), ViolationRateLimit::default());
    let denial = Denial::new(
        None,
        EgressViolationReason::DestinationAddressDenied(trogon_sandbox_contract::AddressDenialClass::CloudMetadata),
    );

    reporter
        .report_outcome(&ProxyOutcome::Denied(denial), "169.254.169.254:80".parse()?)
        .await;

    let events = sink.taken();
    let value: serde_json::Value = serde_json::from_slice(&events[0].1)?;
    assert_eq!(
        value["reason"],
        serde_json::json!({"destination_address_denied": "cloud_metadata"})
    );
    assert_eq!(value["dest_ip"], "169.254.169.254");
    Ok(())
}

#[tokio::test]
async fn an_off_host_sentinel_is_reported_by_binding_name() -> Result<(), Box<dyn std::error::Error>> {
    let sink = Arc::new(CollectingSink::default());
    let reporter = ViolationReporter::new(lease("L1"), sink.clone(), ViolationRateLimit::default());

    reporter
        .report_sentinel_off_host("registry.example.org", 443, &SecretName::new("api")?)
        .await;

    let events = sink.taken();
    assert_eq!(events.len(), 1);
    let json: serde_json::Value = serde_json::from_slice(&events[0].1)?;
    assert_eq!(json["reason"], "secret_sentinel_off_host");
    assert_eq!(json["binding"], "api");
    assert_eq!(json["host"], "registry.example.org");
    Ok(())
}

#[tokio::test]
async fn non_denied_outcomes_publish_nothing() -> Result<(), Box<dyn std::error::Error>> {
    let sink = Arc::new(CollectingSink::default());
    let reporter = ViolationReporter::new(lease("L1"), sink.clone(), ViolationRateLimit::default());

    reporter
        .report_outcome(
            &ProxyOutcome::Completed {
                bytes_client_to_upstream: 1,
                bytes_upstream_to_client: 1,
            },
            dest()?,
        )
        .await;

    assert!(sink.taken().is_empty());
    Ok(())
}

#[tokio::test]
async fn reporter_caps_events_within_the_window() -> Result<(), Box<dyn std::error::Error>> {
    let sink = Arc::new(CollectingSink::default());
    let limit = ViolationRateLimit {
        max_events_per_window: 2,
        window: Duration::from_secs(3600),
        max_tracked_leases: 8,
    };
    let reporter = ViolationReporter::new(lease("L1"), sink.clone(), limit);

    for _ in 0..10 {
        reporter.report(&denial(), dest()?).await;
    }

    assert_eq!(sink.taken().len(), 2);
    Ok(())
}

#[test]
fn limiter_is_per_lease_and_resets_after_the_window() -> Result<(), Box<dyn std::error::Error>> {
    let limit = ViolationRateLimit {
        max_events_per_window: 1,
        window: Duration::from_secs(10),
        max_tracked_leases: 8,
    };
    let mut limiter = ViolationRateLimiter::new(limit);
    let t0 = Instant::now();

    assert!(limiter.admit(&lease("A"), t0));
    assert!(!limiter.admit(&lease("A"), t0));
    assert!(limiter.admit(&lease("B"), t0));
    assert!(limiter.admit(&lease("A"), t0 + Duration::from_secs(10)));
    Ok(())
}

#[test]
fn limiter_memory_is_bounded_by_tracked_lease_cap() -> Result<(), Box<dyn std::error::Error>> {
    let limit = ViolationRateLimit {
        max_events_per_window: 1,
        window: Duration::from_secs(10),
        max_tracked_leases: 2,
    };
    let mut limiter = ViolationRateLimiter::new(limit);
    let t0 = Instant::now();

    assert!(limiter.admit(&lease("A"), t0));
    assert!(limiter.admit(&lease("B"), t0));
    assert!(!limiter.admit(&lease("C"), t0));
    assert_eq!(limiter.tracked_leases(), 2);
    assert!(limiter.admit(&lease("C"), t0 + Duration::from_secs(10)));
    assert_eq!(limiter.tracked_leases(), 1);
    Ok(())
}

struct RefusingSink;

#[async_trait]
impl ViolationSink for RefusingSink {
    async fn publish(&self, _subject: String, _payload: Vec<u8>) -> Result<(), ViolationPublishError> {
        Err(ViolationPublishError("sink closed".to_string()))
    }
}

#[tokio::test]
async fn a_compressed_response_is_reported_by_binding_name() -> Result<(), Box<dyn std::error::Error>> {
    let sink = Arc::new(CollectingSink::default());
    let reporter = ViolationReporter::new(lease("L1"), sink.clone(), ViolationRateLimit::default());

    reporter
        .report_compressed_response("registry.example.org", 443, &SecretName::new("api")?)
        .await;

    let events = sink.taken();
    assert_eq!(events.len(), 1);
    let parsed: EgressViolation = serde_json::from_slice(&events[0].1)?;
    assert_eq!(parsed.reason, EgressViolationReason::SecretCompressedResponse);
    assert_eq!(parsed.binding, Some(SecretName::new("api")?));
    assert_eq!(parsed.host.as_deref(), Some("registry.example.org"));
    Ok(())
}

#[tokio::test]
async fn a_refused_signature_is_reported_with_the_given_reason() -> Result<(), Box<dyn std::error::Error>> {
    let sink = Arc::new(CollectingSink::default());
    let reporter = ViolationReporter::new(lease("L1"), sink.clone(), ViolationRateLimit::default());

    reporter
        .report_sigv4_refused(
            "s3.example.org",
            443,
            &SecretName::new("aws")?,
            EgressViolationReason::PolicyDenied,
        )
        .await;

    let events = sink.taken();
    assert_eq!(events.len(), 1);
    let parsed: EgressViolation = serde_json::from_slice(&events[0].1)?;
    assert_eq!(parsed.reason, EgressViolationReason::PolicyDenied);
    assert_eq!(parsed.binding, Some(SecretName::new("aws")?));
    Ok(())
}

#[tokio::test]
async fn an_event_that_cannot_be_encoded_is_dropped() -> Result<(), Box<dyn std::error::Error>> {
    let sink = Arc::new(CollectingSink::default());
    let reporter = ViolationReporter::new(lease("L1"), sink.clone(), ViolationRateLimit::default());

    reporter
        .emit(EgressViolation {
            lease_id: lease("L1"),
            host: None,
            port: 443,
            dest_ip: None,
            decision: EgressDecision::Deny,
            reason: EgressViolationReason::PolicyDenied,
            binding: None,
            request: None,
            at: SystemTime::UNIX_EPOCH - Duration::from_secs(1),
        })
        .await;

    assert!(sink.taken().is_empty());
    Ok(())
}

#[tokio::test]
async fn a_sink_that_refuses_the_event_does_not_stop_the_report() -> Result<(), Box<dyn std::error::Error>> {
    let reporter = ViolationReporter::new(lease("L1"), Arc::new(RefusingSink), ViolationRateLimit::default());

    reporter.report_outcome(&ProxyOutcome::Denied(denial()), dest()?).await;
    Ok(())
}

#[tokio::test]
async fn the_nats_sink_publishes_on_the_subject_and_refuses_an_oversized_payload()
-> Result<(), Box<dyn std::error::Error>> {
    let server = JetStreamTestServer::start().await;
    let client = server.client().await;
    let mut subscriber = client.subscribe("sandbox.egress.violation.L1".to_string()).await?;
    client.flush().await?;
    let sink = NatsViolationSink::new(client.clone());

    sink.publish("sandbox.egress.violation.L1".to_string(), b"{}".to_vec())
        .await?;
    let message = futures::StreamExt::next(&mut subscriber)
        .await
        .ok_or("no message received")?;
    assert_eq!(message.payload.as_ref(), b"{}");

    let oversized = vec![0u8; client.server_info().max_payload + 1];
    assert!(
        sink.publish("sandbox.egress.violation.L1".to_string(), oversized)
            .await
            .is_err()
    );
    Ok(())
}
