use serde::{Deserialize, Serialize};
use trogon_sandbox_contract::{NativeSnapshotRef, TenantId};
use trogon_sandbox_protocol::subjects::provider_token;

#[derive(Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum SnapshotState {
    Live,
    Deleting,
    Deleted,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct OwnedSnapshot {
    pub(super) version: u8,
    pub(super) tenant: TenantId,
    pub(super) reference: NativeSnapshotRef,
    pub(super) state: SnapshotState,
}

impl OwnedSnapshot {
    pub(super) fn live(tenant: &TenantId, reference: &NativeSnapshotRef) -> Self {
        Self {
            version: 1,
            tenant: tenant.clone(),
            reference: reference.clone(),
            state: SnapshotState::Live,
        }
    }

    pub(super) fn decode(data: &[u8], tenant: &TenantId, reference: &NativeSnapshotRef) -> anyhow::Result<Self> {
        let value: serde_json::Value = serde_json::from_slice(data)?;
        let record = if value.get("version").is_some() {
            serde_json::from_value::<Self>(value)?
        } else {
            let fields = value
                .as_object()
                .ok_or_else(|| anyhow::anyhow!("legacy native snapshot metadata is not an object"))?;
            anyhow::ensure!(
                fields
                    .keys()
                    .all(|key| matches!(key.as_str(), "provider" | "id" | "expires_at")),
                "legacy native snapshot metadata has unrecognized fields"
            );
            Self::live(tenant, &serde_json::from_value(value)?)
        };
        anyhow::ensure!(
            record.version == 1 && &record.tenant == tenant,
            "native snapshot owner or version mismatch"
        );
        anyhow::ensure!(
            record.reference.provider == reference.provider && record.reference.id == reference.id,
            "native snapshot identity mismatch"
        );
        Ok(record)
    }
}

pub(super) fn entry_name(snapshot: &NativeSnapshotRef) -> String {
    format!("{}.{}", provider_token(snapshot.provider), snapshot.id)
}
