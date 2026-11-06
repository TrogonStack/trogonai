#![allow(clippy::expect_used, clippy::panic)]

use std::num::NonZeroU64;
use std::process::Stdio;
use std::time::{Duration, SystemTime};

use buffa::{EnumValue, Message, MessageField};
use buffa_types::google::protobuf::Timestamp;
use ed25519_dalek::{Signer, SigningKey};
use serde_json::json;
use tokio::io::AsyncWriteExt;
use tokio::process::{Child, ChildStdin, Command};
use trogon_audit::{AuditSubject, ChangeTerminal, record_seal};
use trogon_nats::test_support::TlsJetStreamCluster;
use trogon_nats::{NatsConfig, NatsTls, NatsToken, connect_once};
use trogon_workload_registration::{
    ProducerConfig, RegistrationRole, Sha256Digest, StreamProfile, approval_signing_bytes,
};
use trogonai_proto::platform::audit::v1::{
    AuditClass, AuditRecord, ChangeKind, audit_actor::Kind as ActorKind, audit_record::Detail,
};
use trogonai_proto::platform::workload_identity::v1alpha1::{RegistrationBinding, RegistrationSnapshot, WorkloadRole};

fn role_name(role: RegistrationRole, scenario: usize) -> NatsToken {
    let role = match role {
        RegistrationRole::Provisioner => "provisioner",
        RegistrationRole::Publisher => "publisher",
        RegistrationRole::Revoker => "revoker",
    };
    NatsToken::new(format!("{role}_{scenario}")).expect("fixture role")
}

fn command(nats: &NatsConfig, config: &ProducerConfig, operation: &str) -> Command {
    let public_key = SigningKey::from_bytes(&[7; 32]).verifying_key().to_bytes();
    assert_eq!(config.approval_key().fingerprint(), Sha256Digest::hash(&public_key));
    let Ok(NatsTls::Enabled {
        ca: Some(ca),
        client_cert: Some(client),
    }) = &nats.tls
    else {
        panic!("native fixture must supply authenticated verified TLS");
    };
    let mut command = Command::new(env!("CARGO_BIN_EXE_trogon-workload-registration"));
    command
        .env_clear()
        .kill_on_drop(true)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("NATS_URL", nats.servers.join(","))
        .env("NATS_CA", ca.path().expect("native CA path"))
        .env("NATS_CERT", client.cert.path().expect("native certificate path"))
        .env("NATS_KEY", client.key.path().expect("native key path"))
        .args([
            operation,
            "--tenant",
            config.scope().tenant().as_str(),
            "--project",
            config.scope().project().as_str(),
            "--trust-domain",
            config.scope().trust_domain().as_str(),
            "--recovery-fence",
            &config.recovery_fence().to_string(),
            "--approval-public-key",
            &hex::encode(public_key),
        ]);
    super::subprocess_environment::preserve_coverage(command.as_std_mut());
    command
}

enum Effect {
    Grant,
    Denial,
}

fn approval(config: &ProducerConfig, revision: u64, lifetime: Duration, effect: Effect) -> (Vec<u8>, Vec<u8>) {
    let issued = SystemTime::now();
    let payload = RegistrationSnapshot {
        tenant: config.scope().tenant().as_str().into(),
        project: config.scope().project().as_str().into(),
        recovery_fence: config.recovery_fence().get(),
        registration_revision: revision,
        trust_revision: 1,
        issued_at: MessageField::some(Timestamp::from(issued)),
        expires_at: MessageField::some(Timestamp::from(issued + lifetime)),
        bindings: match effect {
            Effect::Denial => vec![],
            Effect::Grant => vec![RegistrationBinding {
                spiffe_id: "spiffe://identity.example.org/service/egress".into(),
                tenant: config.scope().tenant().as_str().into(),
                project: config.scope().project().as_str().into(),
                role: EnumValue::from(WorkloadRole::EgressProxy),
                application_id: None,
            }],
        },
        approved_root_sha256: vec![vec![1; 32]],
    }
    .encode_to_vec();
    let signature = SigningKey::from_bytes(&[7; 32])
        .sign(&approval_signing_bytes(config, &payload))
        .to_bytes();
    let mut frame = u32::try_from(payload.len())
        .expect("bounded fixture frame")
        .to_be_bytes()
        .to_vec();
    frame.extend_from_slice(&signature);
    frame.extend_from_slice(&payload);
    (payload, frame)
}

async fn start(command: &mut Command, frame: &[u8]) -> (Child, ChildStdin) {
    let mut child = command
        .stdin(Stdio::piped())
        .spawn()
        .expect("spawn real registration runner");
    let mut input = child.stdin.take().expect("trusted controller pipe");
    input.write_all(frame).await.expect("write fresh signed approval");
    (child, input)
}

async fn completed_frame(command: &mut Command, frame: &[u8], code: i32) {
    let (mut child, input) = start(command, frame).await;
    drop(input);
    let status = tokio::time::timeout(Duration::from_secs(8), child.wait())
        .await
        .expect("bounded runner completion")
        .expect("reap runner");
    assert_eq!(status.code(), Some(code));
}

async fn live(
    context: &async_nats::jetstream::Context,
    profile: &StreamProfile,
    revision: u64,
    child: Option<&mut Child>,
) -> Vec<u8> {
    let mut child = child;
    tokio::time::timeout(Duration::from_secs(6), async {
        loop {
            if let Some(child) = child.as_mut() {
                assert!(
                    child.try_wait().expect("check active runner").is_none(),
                    "runner stopped before the protected publication"
                );
            }
            if let Ok(stream) = context.get_stream(profile.live_config().name).await
                && let Ok(message) = stream.get_last_raw_message_by_subject(&profile.live_subject()).await
            {
                let snapshot =
                    RegistrationSnapshot::decode_from_slice(&message.payload).expect("actual runtime contract");
                if snapshot.registration_revision == revision {
                    break message.payload.to_vec();
                }
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("acknowledged live snapshot")
}

async fn audited(
    context: &async_nats::jetstream::Context,
    config: &ProducerConfig,
    profile: &StreamProfile,
    payload: &[u8],
    effect: Effect,
) {
    let subject = AuditSubject::change(config.scope().project().clone(), ChangeTerminal::Policy).render();
    let stream = context
        .get_stream(trogon_audit::constants::STREAM_AUDIT)
        .await
        .expect("real shared audit stream");
    let stored = stream
        .get_last_raw_message_by_subject(&subject)
        .await
        .expect("stored policy change acknowledgement");
    record_seal::open(&stored.payload).expect("actual native writer hash seal");
    let record = AuditRecord::decode_from_slice(&stored.payload).expect("stored audit protobuf");
    assert_eq!(record.class.as_known(), Some(AuditClass::Must));
    assert!(record.attribution.is_unset());
    let actor = record.actor.as_option().expect("configured authenticated actor");
    let Some(ActorKind::ApiKey(actor)) = actor.kind.as_ref() else {
        panic!("approval must use its pinned-key actor");
    };
    assert_eq!(
        actor.api_key_id,
        format!("ed25519_sha256:{}", config.approval_key().fingerprint().to_hex())
    );
    let Some(Detail::Change(change)) = record.detail.as_ref() else {
        panic!("expected actual policy change record");
    };
    assert_eq!(change.kind, EnumValue::from(ChangeKind::CHANGE_KIND_POLICY));
    assert_eq!(change.resource_name, profile.live_bucket());
    assert_eq!(
        change.event_id,
        format!("sha256:{}", Sha256Digest::hash(payload).to_hex())
    );
    let (source_stream, source_subject, event_type) = match effect {
        Effect::Grant => (
            profile.control_config().name,
            profile.control_subject(),
            "workload_registration_grant_intent",
        ),
        Effect::Denial => (
            profile.live_config().name,
            profile.live_subject(),
            "workload_registration_withdrawal",
        ),
    };
    assert_eq!(change.event_type, event_type);
    assert_eq!(change.source_stream, source_stream);
    let source = context
        .get_stream(source_stream)
        .await
        .expect("actual recorded source stream")
        .get_last_raw_message_by_subject(&source_subject)
        .await
        .expect("actual recorded source acknowledgement");
    assert_eq!(source.payload.as_ref(), payload);
    assert_eq!(change.stream_sequence, source.sequence);
}

#[tokio::test]
async fn real_runner_uses_scoped_roles_shared_audit_and_terminal_authority_bounds() {
    let key = SigningKey::from_bytes(&[7; 32]);
    let configs: Vec<_> = (1..=3)
        .map(|fence| {
            ProducerConfig::new(
                "acme",
                "payments",
                "identity.example.org",
                NonZeroU64::new(fence).expect("fence"),
                key.verifying_key(),
            )
            .expect("independent pinned profile")
        })
        .collect();
    let profiles: Vec<_> = configs.iter().map(StreamProfile::new).collect();
    let mut users = Vec::new();
    for (index, profile) in profiles.iter().enumerate() {
        for role in [
            RegistrationRole::Provisioner,
            RegistrationRole::Publisher,
            RegistrationRole::Revoker,
        ] {
            users.push((
                role_name(role, index + 1),
                json!({
                    "publish": profile.publish_allowlist(role),
                    "subscribe": profile.subscribe_allowlist(role),
                }),
            ));
        }
    }
    let cluster = TlsJetStreamCluster::start(&users).await;
    let administrator = cluster
        .config(&NatsToken::new("fixture-admin").expect("fixture admin"))
        .expect("native admin identity");
    let reader = connect_once(&administrator, administrator.auth.clone(), Duration::from_secs(5))
        .await
        .expect("verified native reader");
    let context = async_nats::jetstream::new(reader);
    context
        .create_stream(async_nats::jetstream::stream::Config {
            name: trogon_audit::constants::STREAM_AUDIT.into(),
            subjects: vec!["platform.v1.audit.>".into()],
            num_replicas: 3,
            storage: async_nats::jetstream::stream::StorageType::File,
            discard: async_nats::jetstream::stream::DiscardPolicy::New,
            max_bytes: 16 * 1024 * 1024,
            ..Default::default()
        })
        .await
        .expect("separately provision native shared audit");
    for (index, config) in configs.iter().enumerate() {
        let nats = cluster
            .config(&role_name(RegistrationRole::Provisioner, index + 1))
            .expect("scoped provisioner");
        let output = command(&nats, config, "provision")
            .output()
            .await
            .expect("real provision command");
        assert!(output.status.success(), "protected stream provisioning must be usable");
    }

    let config = &configs[0];
    let profile = &profiles[0];
    let publisher = cluster
        .config(&role_name(RegistrationRole::Publisher, 1))
        .expect("scoped publisher");
    let (first, frame) = approval(config, 1, Duration::from_secs(30), Effect::Grant);
    let (mut child, mut input) = start(&mut command(&publisher, config, "serve"), &frame).await;
    assert_eq!(live(&context, profile, 1, Some(&mut child)).await, first);
    audited(&context, config, profile, &first, Effect::Grant).await;
    tokio::time::sleep(Duration::from_secs(6)).await;
    assert!(
        child.try_wait().expect("idle approved runner").is_none(),
        "idle authority must use the lease rather than restart or expire a frame timer"
    );
    assert_eq!(live(&context, profile, 1, Some(&mut child)).await, first);
    let (renewal, frame) = approval(config, 2, Duration::from_secs(30), Effect::Grant);
    input.write_all(&frame).await.expect("independent signed renewal");
    assert_eq!(live(&context, profile, 2, Some(&mut child)).await, renewal);
    audited(&context, config, profile, &renewal, Effect::Grant).await;
    let pid = child.id().expect("owned runner PID").to_string();
    let signal = std::process::Command::new("/bin/kill")
        .args(["-TERM", &pid])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("signal owned runner");
    assert!(signal.success());
    let status = tokio::time::timeout(Duration::from_secs(5), child.wait())
        .await
        .expect("bounded signal cancellation")
        .expect("reap signalled runner");
    assert_eq!(status.code(), Some(6));
    drop(input);
    let (_, frame) = approval(config, 3, Duration::from_secs(30), Effect::Grant);
    completed_frame(&mut command(&publisher, config, "serve"), &frame, 5).await;
    let revoker = cluster
        .config(&role_name(RegistrationRole::Revoker, 1))
        .expect("independent emergency identity");
    let (denial, frame) = approval(config, 3, Duration::from_secs(30), Effect::Denial);
    completed_frame(&mut command(&revoker, config, "withdraw"), &frame, 0).await;
    assert_eq!(live(&context, profile, 3, None).await, denial);
    audited(&context, config, profile, &denial, Effect::Denial).await;

    let config = &configs[1];
    let profile = &profiles[1];
    let publisher = cluster
        .config(&role_name(RegistrationRole::Publisher, 2))
        .expect("separate-fence publisher");
    let (original, frame) = approval(config, 1, Duration::from_secs(8), Effect::Grant);
    let (mut child, input) = start(&mut command(&publisher, config, "serve"), &frame).await;
    assert_eq!(live(&context, profile, 1, Some(&mut child)).await, original);
    let status = tokio::time::timeout(Duration::from_secs(10), child.wait())
        .await
        .expect("finite approved lease")
        .expect("reap expired runner");
    assert_eq!(status.code(), Some(6));
    drop(input);
    assert_eq!(live(&context, profile, 1, None).await, original);
    let (_, frame) = approval(config, 2, Duration::from_secs(30), Effect::Grant);
    completed_frame(&mut command(&publisher, config, "serve"), &frame, 5).await;

    let config = &configs[2];
    let profile = &profiles[2];
    let publisher = cluster
        .config(&role_name(RegistrationRole::Publisher, 3))
        .expect("audit-failure publisher");
    let (original, frame) = approval(config, 1, Duration::from_secs(30), Effect::Grant);
    let (mut child, mut input) = start(&mut command(&publisher, config, "serve"), &frame).await;
    assert_eq!(live(&context, profile, 1, Some(&mut child)).await, original);
    context
        .delete_stream(trogon_audit::constants::STREAM_AUDIT)
        .await
        .expect("actual audit outage");
    let (_, frame) = approval(config, 2, Duration::from_secs(30), Effect::Grant);
    input
        .write_all(&frame)
        .await
        .expect("signed change during audit outage");
    let status = tokio::time::timeout(Duration::from_secs(8), child.wait())
        .await
        .expect("bounded failed audit operation")
        .expect("reap failed runner");
    assert_eq!(status.code(), Some(5));
    drop(input);
    assert_eq!(
        live(&context, profile, 1, None).await,
        original,
        "audit outage must not promote authority"
    );
}
