use std::sync::Mutex;
use std::time::SystemTime;

use trogon_sandbox_contract::{LeaseId, ProviderKind, WorkerId};
use trogon_sandbox_protocol::{DispatchVerifier, EnforcementMode};
use trogon_std::env::InMemoryEnv;

use super::*;
use crate::active_lease::InMemoryActiveLeaseStore;

type TestResult = Result<(), Box<dyn std::error::Error>>;
type Sent = (String, Option<String>, Bytes);

#[derive(Default)]
struct Recorder {
    sent: Mutex<Vec<Sent>>,
}

impl Recorder {
    fn push(&self, subject: &str, auth: Option<&str>, payload: Bytes) {
        self.sent.lock().unwrap_or_else(|e| e.into_inner()).push((
            subject.to_owned(),
            auth.map(str::to_owned),
            payload,
        ));
    }

    fn sent(&self) -> Vec<Sent> {
        self.sent.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

#[async_trait]
impl BrokerBus for Recorder {
    async fn request(&self, subject: &str, payload: Bytes) -> anyhow::Result<Bytes> {
        self.push(subject, None, payload);
        Ok(Bytes::new())
    }
    async fn publish(&self, subject: &str, payload: Bytes) -> anyhow::Result<()> {
        self.request(subject, payload).await.map(|_| ())
    }
    async fn request_with_auth(
        &self,
        subject: &str,
        auth: &str,
        payload: Bytes,
        _timeout: Option<Duration>,
    ) -> anyhow::Result<Bytes> {
        self.push(subject, Some(auth), payload);
        Ok(Bytes::new())
    }
}

fn seed(byte: u8) -> [u8; 32] {
    [byte; 32]
}

async fn setup() -> Result<(Arc<Recorder>, SigningBus, DispatchVerifier), Box<dyn std::error::Error>> {
    let recorder = Arc::new(Recorder::default());
    let leases = Arc::new(InMemoryActiveLeaseStore::new());
    let mut record =
        crate::active_lease::tests::sample_record("l1", "t1", SystemTime::now() + Duration::from_secs(60))?;
    record.worker_id = Some(WorkerId::new("w1")?);
    leases.put(&record).await?;
    let signer = DispatchSigner::new("k1", seed(7));
    let verifier = DispatchVerifier::new(EnforcementMode::Require).with_key("k1", &signer.public_key_b64())?;
    let bus = SigningBus::new(recorder.clone(), leases, Arc::new(signer));
    Ok((recorder, bus, verifier))
}

fn first_auth(recorder: &Recorder, index: usize) -> Result<String, Box<dyn std::error::Error>> {
    recorder
        .sent()
        .get(index)
        .and_then(|sent| sent.1.clone())
        .ok_or_else(|| "request was not signed".into())
}

#[tokio::test]
async fn a_lease_verb_is_signed_for_its_tenant() -> TestResult {
    let (recorder, bus, verifier) = setup().await?;
    let subject = "sandbox.provider.mock.run_command.l1";
    bus.request(subject, Bytes::from_static(b"{}")).await?;
    let auth = first_auth(&recorder, 0)?;
    assert!(
        verifier
            .verify_verb(subject, &auth, b"{}", false, SystemTime::now())
            .is_ok()
    );
    let parsed: serde_json::Value = serde_json::from_str(&auth)?;
    assert_eq!(parsed["tenant"], "t1");
    assert_eq!(parsed["worker_id"], "w1");
    Ok(())
}

#[tokio::test]
async fn native_snapshot_deletion_is_signed_without_a_live_lease() -> TestResult {
    let (recorder, bus, verifier) = setup().await?;
    let subject = trogon_sandbox_protocol::subjects::provider_native_snapshot_delete(ProviderKind::E2b);
    let body =
        Bytes::from_static(br#"{"tenant":"snapshot-owner","reference":{"provider":"e2b","id":"opaque-capture"}}"#);
    bus.request(&subject, body.clone()).await?;
    let header = first_auth(&recorder, 0)?;
    assert!(
        verifier
            .verify_verb(&subject, &header, &body, false, SystemTime::now())
            .is_ok()
    );
    let auth: serde_json::Value = serde_json::from_str(&header)?;
    assert_eq!(auth["tenant"], "snapshot-owner");
    assert_eq!(auth["lease_id"], "");
    assert!(auth.get("worker_id").is_none_or(serde_json::Value::is_null));
    Ok(())
}

#[tokio::test]
async fn native_snapshot_deletion_for_another_provider_is_unsigned() -> TestResult {
    let (recorder, bus, _) = setup().await?;
    let subject = trogon_sandbox_protocol::subjects::provider_native_snapshot_delete(ProviderKind::LocalDocker);
    let body =
        Bytes::from_static(br#"{"tenant":"snapshot-owner","reference":{"provider":"e2b","id":"opaque-capture"}}"#);
    bus.request(&subject, body).await?;
    assert!(recorder.sent().iter().all(|sent| sent.1.is_none()));
    Ok(())
}

#[tokio::test]
async fn owned_cancel_reconstruction_signs_only_exact_typed_custody() -> TestResult {
    let recorder = Arc::new(Recorder::default());
    let signer = Arc::new(DispatchSigner::new("owned-cleanup-test", seed(11)));
    let verifier =
        DispatchVerifier::new(EnforcementMode::Require).with_key("owned-cleanup-test", &signer.public_key_b64())?;
    let bus = SigningBus::new(recorder.clone(), Arc::new(InMemoryActiveLeaseStore::new()), signer);
    let subject = "sandbox.provider.e2b.cancel_owned.retired-lease";
    let payload =
        Bytes::from_static(br#"{"tenant":"owner","lease_id":"retired-lease","sandbox_id":"canonical-native-id"}"#);
    bus.request(subject, payload.clone()).await?;
    let header = first_auth(&recorder, 0)?;
    assert!(
        verifier
            .verify_verb(subject, &header, &payload, false, SystemTime::now())
            .is_ok()
    );
    for (subject, payload) in [
        (
            subject,
            br#"{"tenant":"owner","lease_id":"another-lease","sandbox_id":"canonical-native-id"}"#.as_slice(),
        ),
        (
            subject,
            br#"{"tenant":"","lease_id":"retired-lease","sandbox_id":"canonical-native-id"}"#.as_slice(),
        ),
        (
            "sandbox.provider.e2b.kill_command.retired-lease",
            br#"{"tenant":"owner","lease_id":"retired-lease","sandbox_id":"canonical-native-id"}"#.as_slice(),
        ),
    ] {
        bus.request(subject, Bytes::copy_from_slice(payload)).await?;
    }
    assert!(recorder.sent()[1..].iter().all(|(_, header, _)| header.is_none()));
    Ok(())
}

#[tokio::test]
async fn unrelated_subjects_and_unknown_leases_are_unsigned() -> TestResult {
    let (recorder, bus, _) = setup().await?;
    bus.request("sandbox.lease.request", Bytes::new()).await?;
    bus.request("sandbox.provider.mock.run_command.gone", Bytes::new())
        .await?;
    assert!(recorder.sent().iter().all(|s| s.1.is_none()));
    Ok(())
}

#[tokio::test]
async fn a_released_lease_is_still_signed_for_its_remembered_tenant() -> TestResult {
    let (recorder, bus, verifier) = setup().await?;
    let subject = "sandbox.provider.mock.pause.l1";
    bus.request(subject, Bytes::from_static(b"{}")).await?;
    bus.leases.delete(&LeaseId::new("l1")).await?;
    bus.request(subject, Bytes::from_static(b"{}")).await?;
    let auth = first_auth(&recorder, 1)?;
    let parsed: serde_json::Value = serde_json::from_str(&auth)?;
    assert_eq!(parsed["tenant"], "t1");
    assert!(parsed.get("worker_id").is_none_or(|w| w.is_null()));
    assert!(
        verifier
            .verify_verb(subject, &auth, b"{}", false, SystemTime::now())
            .is_ok()
    );
    Ok(())
}

#[tokio::test]
async fn cleanup_and_purge_are_signed() -> TestResult {
    let (recorder, bus, _) = setup().await?;
    let body = Bytes::from(serde_json::to_vec(&serde_json::json!({
        "identity": {"lease_id": "l1", "provider": "mock",
            "target": {"worker_id": "w1", "attempt_id": "a1"}},
        "intent": "release",
    }))?);
    bus.request_with_timeout("sandbox.provider.mock.worker.w1.cleanup", body, Duration::from_secs(1))
        .await?;
    bus.request(
        "sandbox.provider.substrate.purge_named",
        Bytes::from_static(br#"{"tenant":"t9","name":"n"}"#),
    )
    .await?;
    let cleanup: serde_json::Value = serde_json::from_str(&first_auth(&recorder, 0)?)?;
    assert_eq!(cleanup["lease_id"], "l1");
    assert_eq!(cleanup["tenant"], "t1");
    let purge: serde_json::Value = serde_json::from_str(&first_auth(&recorder, 1)?)?;
    assert_eq!(purge["tenant"], "t9");
    Ok(())
}

#[tokio::test]
async fn a_cleanup_for_a_forgotten_lease_carries_the_remembered_attempt() -> TestResult {
    let (recorder, bus, _) = setup().await?;
    bus.request("sandbox.provider.mock.pause.l1", Bytes::from_static(b"{}"))
        .await?;
    bus.leases.delete(&LeaseId::new("l1")).await?;
    let body = Bytes::from(serde_json::to_vec(&serde_json::json!({
        "identity": {"lease_id": "l1", "provider": "mock",
            "target": {"worker_id": "w1", "attempt_id": "a1"}},
    }))?);
    bus.request("sandbox.provider.mock.worker.w1.cleanup", body).await?;
    let cleanup: serde_json::Value = serde_json::from_str(&first_auth(&recorder, 1)?)?;
    assert_eq!(cleanup["attempt_id"], "a1");
    Ok(())
}

#[tokio::test]
async fn the_remembered_tenants_are_bounded() -> TestResult {
    let (_, bus, _) = setup().await?;
    for index in 0..=REMEMBERED_LEASES {
        bus.remember(&LeaseId::new(format!("lease-{index}")), &TenantId::new("t")?);
    }
    assert!(bus.remembered(&LeaseId::new("lease-0")).is_none());
    assert!(
        bus.remembered(&LeaseId::new(format!("lease-{REMEMBERED_LEASES}")))
            .is_some()
    );
    Ok(())
}

#[tokio::test]
async fn the_other_sends_pass_through_to_the_wrapped_bus() -> TestResult {
    let (recorder, bus, _) = setup().await?;
    bus.publish_with_msg_id("sandbox.event", "id", Bytes::new()).await?;
    bus.request_with_auth("sandbox.event", "auth", Bytes::new(), None)
        .await?;
    bus.publish_with_auth("sandbox.event", "auth", Bytes::new()).await?;
    bus.publish("sandbox.provider.mock.pause.l1", Bytes::from_static(b"{}"))
        .await?;
    assert_eq!(bus.max_payload(), None);
    assert_eq!(recorder.sent().len(), 4);
    Ok(())
}

#[tokio::test]
async fn timeouts_are_kept_for_unsigned_requests() -> TestResult {
    let (recorder, bus, _) = setup().await?;
    bus.request_with_timeout("sandbox.lease.request", Bytes::new(), Duration::from_secs(1))
        .await?;
    assert_eq!(recorder.sent().len(), 1);
    Ok(())
}

#[test]
fn a_signing_key_in_the_environment_wraps_the_bus() -> TestResult {
    let leases: Arc<dyn ActiveLeaseStore> = Arc::new(InMemoryActiveLeaseStore::new());
    let unconfigured = wrap_from_env(&InMemoryEnv::new(), Recorder::default(), &leases)?;
    assert_eq!(unconfigured.max_payload(), None);
    let (seed_b64, _) = DispatchSigner::generate_keypair_b64();
    let env = InMemoryEnv::new();
    env.set(trogon_sandbox_protocol::ENV_SIGNING_KEY, &seed_b64);
    env.set(trogon_sandbox_protocol::ENV_SIGNING_KEY_ID, "k1");
    let _configured = wrap_from_env(&env, Recorder::default(), &leases)?;
    let broken = InMemoryEnv::new();
    broken.set(trogon_sandbox_protocol::ENV_SIGNING_KEY, "not a seed");
    assert!(matches!(
        wrap_from_env(&broken, Recorder::default(), &leases),
        Err(SigningBusError::Misconfigured(_))
    ));
    Ok(())
}
