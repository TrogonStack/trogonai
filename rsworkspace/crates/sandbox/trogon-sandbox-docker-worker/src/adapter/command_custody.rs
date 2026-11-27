use anyhow::ensure;
use async_nats::jetstream::{self, kv, stream};
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use tokio::sync::OnceCell;
use trogon_sandbox_contract::DispatchTarget;
use trogon_sandbox_contract::{CommandLaunchOrigin, ExecutionId, LaunchKey, LeaseId, TenantId};
use trogon_sandbox_protocol::tenant_scoped_kv_key;

use super::constants::{COMMAND_CUSTODY_BUCKET, MAX_CUSTODY_RECORD_BYTES};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Owner {
    pub tenant: TenantId,
    pub lease_id: LeaseId,
    pub container_id: String,
    pub worker_id: String,
    pub registration_id: String,
    pub target: Option<DispatchTarget>,
}

impl Owner {
    fn validate(&self) -> anyhow::Result<()> {
        for value in [self.tenant.as_str(), self.lease_id.as_str(), &self.worker_id] {
            ensure!(
                !value.is_empty() && value.len() <= 1024 && !value.chars().any(char::is_control),
                "invalid command owner identity"
            );
        }
        ensure!(
            self.container_id.len() == 64 && self.container_id.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "command custody requires a canonical container ID"
        );
        ensure!(
            crate::fresh_id::is_fresh_id(&self.registration_id),
            "invalid command registration"
        );
        if let Some(target) = &self.target {
            ensure!(
                target.worker_id.as_str() == self.worker_id && target.epoch.is_none_or(|epoch| epoch.get() > 0),
                "command assignment differs from its actual worker"
            );
        }
        Ok(())
    }

    fn key(&self, key: &LaunchKey) -> anyhow::Result<String> {
        self.validate()?;
        Ok(tenant_scoped_kv_key(
            &self.tenant,
            &serde_json::to_string(&(self.lease_id.as_str(), key.as_str()))?,
        ))
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct NativeIdentity {
    pub helper_pid: u32,
    pub helper_start_time: u64,
    pub root_pid: u32,
    pub root_start_time: u64,
    pub process_group: u32,
    pub session: u32,
    pub artifact_digest: String,
    pub protocol: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Absence {
    pub released: bool,
    pub root_exit_code: Option<i32>,
    pub root_signal: Option<i32>,
    pub timed_out: bool,
    pub canceled: bool,
    pub reaped_count: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Phase {
    Reserved,
    Preparing,
    Bound,
    Running,
    Canceled,
    Preserved,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Record {
    version: u32,
    pub owner: Owner,
    pub launch_key: LaunchKey,
    pub fingerprint: Option<String>,
    pub execution_id: Option<ExecutionId>,
    pub origin: Option<CommandLaunchOrigin>,
    pub native: Option<NativeIdentity>,
    pub absence: Option<Absence>,
    pub phase: Phase,
    pub cancel_requested: bool,
}

impl Record {
    pub fn reserved(
        owner: Owner,
        key: &LaunchKey,
        fingerprint: String,
        execution_id: ExecutionId,
        reused: bool,
    ) -> Self {
        Self {
            version: 2,
            owner,
            launch_key: key.clone(),
            fingerprint: Some(fingerprint),
            execution_id: Some(execution_id),
            origin: Some(if reused {
                CommandLaunchOrigin::Reused
            } else {
                CommandLaunchOrigin::Created
            }),
            native: None,
            absence: None,
            phase: if reused { Phase::Preserved } else { Phase::Reserved },
            cancel_requested: false,
        }
    }

    pub fn canceled_before_start(owner: Owner, key: &LaunchKey) -> Self {
        Self {
            version: 2,
            owner,
            launch_key: key.clone(),
            fingerprint: None,
            execution_id: None,
            origin: None,
            native: None,
            absence: None,
            phase: Phase::Canceled,
            cancel_requested: true,
        }
    }

    fn validate(&self) -> anyhow::Result<()> {
        ensure!(self.version == 2, "unsupported command custody version");
        self.owner.validate()?;
        if let Some(execution) = &self.execution_id {
            ensure!(
                crate::fresh_id::is_fresh_id(execution.as_str()),
                "invalid command execution identity"
            );
        }
        if let Some(fingerprint) = &self.fingerprint {
            ensure!(
                fingerprint.len() == 64 && fingerprint.bytes().all(|byte| byte.is_ascii_hexdigit()),
                "invalid command fingerprint"
            );
        }
        if let Some(native) = &self.native {
            ensure!(
                native.helper_pid > 1
                    && native.root_pid > 1
                    && native.helper_pid != native.root_pid
                    && native.helper_start_time > 0
                    && native.root_start_time > 0
                    && native.process_group == native.root_pid
                    && native.session == native.root_pid
                    && native.protocol == sandbox_command_supervisor::protocol::VERSION
                    && native.artifact_digest.len() == 64
                    && native.artifact_digest.bytes().all(|byte| byte.is_ascii_hexdigit()),
                "invalid native supervisor binding"
            );
        }
        if let Some(absence) = &self.absence {
            ensure!(
                self.native.is_some()
                    && self.phase == Phase::Canceled
                    && absence.reaped_count > 0
                    && ((absence.released && (absence.root_exit_code.is_some() ^ absence.root_signal.is_some()))
                        || (!absence.released
                            && absence.root_exit_code.is_none()
                            && absence.root_signal.is_none()
                            && absence.canceled))
                    && absence.root_exit_code.is_none_or(|code| (0..=255).contains(&code))
                    && absence.root_signal.is_none_or(|signal| (1..=64).contains(&signal)),
                "invalid native family absence receipt"
            );
        }
        let bound = self.execution_id.is_some() && self.fingerprint.is_some();
        let valid = match self.phase {
            Phase::Reserved | Phase::Preparing => {
                bound && self.origin == Some(CommandLaunchOrigin::Created) && self.native.is_none()
            }
            Phase::Bound | Phase::Running => {
                bound && self.origin == Some(CommandLaunchOrigin::Created) && self.native.is_some()
            }
            Phase::Preserved => bound && self.origin == Some(CommandLaunchOrigin::Reused) && self.native.is_none(),
            Phase::Canceled => {
                self.cancel_requested
                    && ((bound && self.origin == Some(CommandLaunchOrigin::Created))
                        || (self.execution_id.is_none()
                            && self.fingerprint.is_none()
                            && self.origin.is_none()
                            && self.native.is_none()))
            }
        };
        ensure!(
            valid && (self.phase != Phase::Canceled || self.native.is_some() == self.absence.is_some()),
            "command custody phase lacks immutable authority"
        );
        Ok(())
    }

    fn bytes(&self) -> anyhow::Result<Bytes> {
        self.validate()?;
        let bytes = serde_json::to_vec(self)?;
        ensure!(
            bytes.len() <= MAX_CUSTODY_RECORD_BYTES,
            "command custody exceeds its byte limit"
        );
        Ok(bytes.into())
    }
}

pub(super) struct Stored {
    pub record: Record,
    pub revision: u64,
}

pub(super) struct Custody {
    context: jetstream::Context,
    store: OnceCell<kv::Store>,
}

impl Custody {
    pub fn new(context: jetstream::Context) -> Self {
        Self {
            context,
            store: OnceCell::new(),
        }
    }

    async fn storage(&self) -> anyhow::Result<&kv::Store> {
        let store = self
            .store
            .get_or_try_init(|| async {
                if let Ok(store) = self.context.get_key_value(COMMAND_CUSTODY_BUCKET).await {
                    return Ok::<_, anyhow::Error>(store);
                }
                match self
                    .context
                    .create_key_value(kv::Config {
                        bucket: COMMAND_CUSTODY_BUCKET.into(),
                        history: 1,
                        max_value_size: MAX_CUSTODY_RECORD_BYTES as i32,
                        ..Default::default()
                    })
                    .await
                {
                    Ok(store) => Ok(store),
                    Err(create) => self
                        .context
                        .get_key_value(COMMAND_CUSTODY_BUCKET)
                        .await
                        .map_err(|load| anyhow::anyhow!("command custody unavailable: create={create}; load={load}")),
                }
            })
            .await?;
        let mut native = store.stream.clone();
        let info = native.info().await?;
        ensure!(
            matches!(&info.config, stream::Config { max_age, discard: stream::DiscardPolicy::New, retention: stream::RetentionPolicy::Limits, storage: stream::StorageType::File, allow_message_ttl: false, subject_delete_marker_ttl: None, subject_transform: None, republish: None, sealed: false, no_ack: false, mirror: None, subjects, sources, .. } if max_age.is_zero() && sources.as_ref().is_none_or(Vec::is_empty) && subjects == &[format!("$KV.{COMMAND_CUSTODY_BUCKET}.>")]),
            "command custody requires nonexpiring file authority without eviction"
        );
        Ok(store)
    }

    pub async fn load(&self, owner: &Owner, key: &LaunchKey) -> anyhow::Result<Option<Stored>> {
        let Some(entry) = self.storage().await?.entry(owner.key(key)?).await? else {
            return Ok(None);
        };
        ensure!(
            entry.operation == kv::Operation::Put && entry.value.len() <= MAX_CUSTODY_RECORD_BYTES,
            "unreadable command authority marker"
        );
        let record: Record = serde_json::from_slice(&entry.value)?;
        record.validate()?;
        ensure!(
            record.owner == *owner && record.launch_key == *key,
            "command owner, container or registration changed"
        );
        Ok(Some(Stored {
            record,
            revision: entry.revision,
        }))
    }

    pub async fn create(&self, record: Record) -> anyhow::Result<Option<Stored>> {
        match self
            .storage()
            .await?
            .create(record.owner.key(&record.launch_key)?, record.bytes()?)
            .await
        {
            Ok(revision) => Ok(Some(Stored { record, revision })),
            Err(error) if error.kind() == kv::CreateErrorKind::AlreadyExists => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    pub async fn replace(&self, expected: &Stored, next: Record) -> anyhow::Result<Option<Stored>> {
        ensure!(
            expected.record.owner == next.owner
                && expected.record.launch_key == next.launch_key
                && expected.record.execution_id == next.execution_id
                && expected.record.fingerprint == next.fingerprint
                && expected.record.origin == next.origin
                && expected
                    .record
                    .native
                    .as_ref()
                    .is_none_or(|native| next.native.as_ref() == Some(native))
                && expected
                    .record
                    .absence
                    .as_ref()
                    .is_none_or(|absence| next.absence.as_ref() == Some(absence))
                && (next.phase != Phase::Canceled
                    || expected.record.phase == Phase::Reserved
                    || expected.record.native.is_some())
                && (!expected.record.cancel_requested || next.cancel_requested),
            "immutable command authority changed"
        );
        match self
            .storage()
            .await?
            .update(next.owner.key(&next.launch_key)?, next.bytes()?, expected.revision)
            .await
        {
            Ok(revision) => Ok(Some(Stored { record: next, revision })),
            Err(error) if error.kind() == kv::UpdateErrorKind::WrongLastRevision => Ok(None),
            Err(error) => Err(error.into()),
        }
    }
}
