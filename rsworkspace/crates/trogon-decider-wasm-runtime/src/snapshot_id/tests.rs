use super::*;
use crate::OpaqueSnapshotPayload;
use trogon_decider_runtime::{Snapshot, SnapshotWrite, StreamPosition, WriteSnapshotRequest, WriteSnapshotResponse};

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("invalid NATS Key/Value key")]
struct NatsKvKeyContractError;

#[derive(Debug, Clone, Copy)]
struct NatsKvKeyContractSnapshotStore;

impl SnapshotWrite<OpaqueSnapshotPayload, str> for NatsKvKeyContractSnapshotStore {
    type Error = NatsKvKeyContractError;

    async fn write_snapshot(
        &self,
        request: WriteSnapshotRequest<'_, OpaqueSnapshotPayload, str>,
    ) -> Result<WriteSnapshotResponse, Self::Error> {
        let key = trogon_decider_nats::snapshot_key::<OpaqueSnapshotPayload>(request.snapshot_id)
            .map_err(|_| NatsKvKeyContractError)?;
        let valid = !key.is_empty()
            && !key.starts_with('.')
            && !key.ends_with('.')
            && key
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '/' | '_' | '=' | '.'));
        valid.then_some(WriteSnapshotResponse).ok_or(NatsKvKeyContractError)
    }
}

#[test]
fn renders_module_and_stream_identity() {
    let module_name = ModuleName::new("scheduler.schedules").unwrap();
    let module_version = ModuleVersion::new("0.1.0").unwrap();
    let snapshot_id = WasmSnapshotId::new(&module_name, &module_version, "alpha");
    let expected = "v1_c2NoZWR1bGVyLnNjaGVkdWxlc0AwLjEuMC9hbHBoYQ";

    assert_eq!(snapshot_id.as_str(), expected);
    assert_eq!(snapshot_id.to_string(), expected);
    assert_eq!(snapshot_id.as_ref(), expected);
    assert_eq!(
        <WasmSnapshotId as std::borrow::Borrow<str>>::borrow(&snapshot_id),
        expected
    );
}

#[test]
fn arbitrary_identity_content_uses_a_nats_key_safe_alphabet() {
    let module_name = ModuleName::new("scheduler schedules").unwrap();
    let module_version = ModuleVersion::new("release+candidate").unwrap();
    let snapshot_id = WasmSnapshotId::new(&module_name, &module_version, "orders/東京 @ west");

    assert!(snapshot_id.as_str().starts_with("v1_"));
    assert!(
        snapshot_id
            .as_str()
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    );
}

#[test]
fn version_bump_changes_identity() {
    let module_name = ModuleName::new("scheduler.schedules").unwrap();
    let previous = WasmSnapshotId::new(&module_name, &ModuleVersion::new("0.1.0").unwrap(), "alpha");
    let next = WasmSnapshotId::new(&module_name, &ModuleVersion::new("0.2.0").unwrap(), "alpha");

    assert_ne!(previous, next);
}

#[test]
fn encoded_identity_preserves_module_version_and_stream_component_separation() {
    let module_name = ModuleName::new("scheduler.schedules").unwrap();
    let other_module_name = ModuleName::new("scheduler.schedules.v2").unwrap();
    let module_version = ModuleVersion::new("0.1.0").unwrap();
    let other_module_version = ModuleVersion::new("0.1.0+candidate").unwrap();
    let identities = [
        WasmSnapshotId::new(&module_name, &module_version, "orders/west@1"),
        WasmSnapshotId::new(&other_module_name, &module_version, "orders/west@1"),
        WasmSnapshotId::new(&module_name, &other_module_version, "orders/west@1"),
        WasmSnapshotId::new(&module_name, &module_version, "orders/west@2"),
    ];
    let unique = identities
        .iter()
        .map(WasmSnapshotId::as_str)
        .collect::<std::collections::HashSet<_>>();

    assert_eq!(unique.len(), identities.len());
}

#[tokio::test]
async fn nats_key_contract_rejects_the_legacy_identity_and_accepts_the_encoded_identity() {
    let module_name = ModuleName::new("scheduler.schedules").unwrap();
    let module_version = ModuleVersion::new("0.1.0").unwrap();
    let store = NatsKvKeyContractSnapshotStore;
    let snapshot = Snapshot::new(
        StreamPosition::try_new(1).unwrap(),
        OpaqueSnapshotPayload::new(vec![1, 2, 3]),
    );
    let legacy_id = "scheduler.schedules@0.1.0/alpha";

    let legacy_result = store
        .write_snapshot(WriteSnapshotRequest {
            snapshot_id: legacy_id,
            snapshot: snapshot.clone(),
        })
        .await;
    assert_eq!(legacy_result.unwrap_err(), NatsKvKeyContractError);

    let encoded_id = WasmSnapshotId::new(&module_name, &module_version, "alpha");
    store
        .write_snapshot(WriteSnapshotRequest {
            snapshot_id: encoded_id.as_str(),
            snapshot,
        })
        .await
        .unwrap();
}
