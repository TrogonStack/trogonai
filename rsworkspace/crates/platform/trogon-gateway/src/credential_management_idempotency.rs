use std::error::Error;
use std::sync::Arc;
use std::time::Duration;

#[cfg(test)]
use std::collections::BTreeMap;

use async_nats::jetstream::{self, context, kv};
use buffa::{EnumValue, Message as _, MessageField};
use bytes::Bytes;
use sha2::{Digest, Sha256};
#[cfg(test)]
use tokio::sync::Mutex;
use trogon_nats::jetstream::{
    JetStreamCreateKeyValue, JetStreamGetKeyValue, JetStreamKeyValueDeleteExpectRevision, JetStreamKeyValueStatus,
    JetStreamKeyValueUpdate, JetStreamKvCreate, JetStreamKvEntry, is_create_key_value_already_exists,
};
use trogonai_proto::gateway::credentials::idempotency_v1 as proto;

use crate::credential_management::{CredentialCommandResponse, CredentialRefResponse};

const IDEMPOTENCY_KEY_PREFIX: &str = "v1.";
pub(crate) const CREDENTIAL_MANAGEMENT_IDEMPOTENCY_BUCKET: &str = "GATEWAY_CREDENTIAL_MANAGEMENT_IDEMPOTENCY";
pub(crate) const CREDENTIAL_MANAGEMENT_IDEMPOTENCY_MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct IdempotencyKey(Arc<str>);

impl IdempotencyKey {
    pub(crate) fn new(value: impl AsRef<str>) -> Result<Self, IdempotencyKeyError> {
        let value = value.as_ref();
        let char_count = value.chars().count();
        if char_count == 0 {
            return Err(IdempotencyKeyError::Empty);
        }
        if char_count > 256 {
            return Err(IdempotencyKeyError::TooLong(char_count));
        }
        Ok(Self(Arc::from(value)))
    }

    fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum IdempotencyKeyError {
    #[error("idempotency key must not be empty")]
    Empty,
    #[error("idempotency key exceeds maximum length: {0}")]
    TooLong(usize),
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct IdempotencyScope {
    owner_id: String,
    command_namespace: &'static str,
    target_resource_id: String,
    idempotency_key: IdempotencyKey,
}

impl IdempotencyScope {
    pub(crate) fn new(
        owner_id: &str,
        command_namespace: &'static str,
        target_resource_id: &str,
        idempotency_key: IdempotencyKey,
    ) -> Self {
        Self {
            owner_id: owner_id.to_string(),
            command_namespace,
            target_resource_id: target_resource_id.to_string(),
            idempotency_key,
        }
    }

    fn storage_key(&self) -> String {
        let mut hasher = Sha256::new();
        update_hash(&mut hasher, self.owner_id.as_str());
        update_hash(&mut hasher, self.command_namespace);
        update_hash(&mut hasher, self.target_resource_id.as_str());
        update_hash(&mut hasher, self.idempotency_key.as_str());
        format!("{IDEMPOTENCY_KEY_PREFIX}{}", hex::encode(hasher.finalize()))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RequestFingerprint(String);

impl RequestFingerprint {
    pub(crate) fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

#[derive(Clone, Debug)]
pub(crate) enum IdempotencyDecision {
    Execute,
    Replay(CredentialCommandResponse),
    Conflict,
    InProgress,
}

pub(crate) trait CredentialCommandIdempotencyStore: Clone + Send + Sync + 'static {
    fn begin(
        &self,
        scope: IdempotencyScope,
        fingerprint: RequestFingerprint,
    ) -> impl std::future::Future<Output = Result<IdempotencyDecision, CredentialCommandIdempotencyStoreError>> + Send;

    fn complete(
        &self,
        scope: &IdempotencyScope,
        response: CredentialCommandResponse,
    ) -> impl std::future::Future<Output = Result<(), CredentialCommandIdempotencyStoreError>> + Send;

    fn abandon(
        &self,
        scope: &IdempotencyScope,
    ) -> impl std::future::Future<Output = Result<(), CredentialCommandIdempotencyStoreError>> + Send;
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum CredentialCommandIdempotencyStoreError {
    #[error("credential command idempotency codec failed: {source}")]
    Codec {
        #[source]
        source: Box<dyn Error + Send + Sync>,
    },
    #[error("credential command idempotency backend failed: {source}")]
    Backend {
        #[source]
        source: Box<dyn Error + Send + Sync>,
    },
    #[error("credential command idempotency record changed concurrently")]
    Conflict,
}

impl CredentialCommandIdempotencyStoreError {
    fn backend(source: impl Error + Send + Sync + 'static) -> Self {
        Self::Backend {
            source: Box::new(source),
        }
    }

    fn codec(source: impl Error + Send + Sync + 'static) -> Self {
        Self::Codec {
            source: Box::new(source),
        }
    }

    fn invalid_record(reason: &'static str) -> Self {
        Self::codec(std::io::Error::new(std::io::ErrorKind::InvalidData, reason))
    }
}

#[derive(Clone, Default)]
#[cfg(test)]
pub(crate) struct CredentialCommandInMemoryIdempotencyLedger {
    records: Arc<Mutex<BTreeMap<IdempotencyScope, InMemoryRecord>>>,
}

#[derive(Clone, Debug)]
#[cfg(test)]
enum InMemoryRecord {
    InProgress {
        fingerprint: RequestFingerprint,
    },
    Completed {
        fingerprint: RequestFingerprint,
        response: CredentialCommandResponse,
    },
}

#[cfg(test)]
impl CredentialCommandIdempotencyStore for CredentialCommandInMemoryIdempotencyLedger {
    async fn begin(
        &self,
        scope: IdempotencyScope,
        fingerprint: RequestFingerprint,
    ) -> Result<IdempotencyDecision, CredentialCommandIdempotencyStoreError> {
        let mut records = self.records.lock().await;
        match records.get(&scope) {
            Some(InMemoryRecord::Completed {
                fingerprint: existing,
                response,
            }) if existing == &fingerprint => Ok(IdempotencyDecision::Replay(response.clone())),
            Some(InMemoryRecord::Completed { .. }) => Ok(IdempotencyDecision::Conflict),
            Some(InMemoryRecord::InProgress { fingerprint: existing }) if existing == &fingerprint => {
                Ok(IdempotencyDecision::InProgress)
            }
            Some(InMemoryRecord::InProgress { .. }) => Ok(IdempotencyDecision::Conflict),
            None => {
                records.insert(scope, InMemoryRecord::InProgress { fingerprint });
                Ok(IdempotencyDecision::Execute)
            }
        }
    }

    async fn complete(
        &self,
        scope: &IdempotencyScope,
        response: CredentialCommandResponse,
    ) -> Result<(), CredentialCommandIdempotencyStoreError> {
        let mut records = self.records.lock().await;
        let Some(InMemoryRecord::InProgress { fingerprint }) = records.get(scope).cloned() else {
            return Ok(());
        };
        records.insert(scope.clone(), InMemoryRecord::Completed { fingerprint, response });
        Ok(())
    }

    async fn abandon(&self, scope: &IdempotencyScope) -> Result<(), CredentialCommandIdempotencyStoreError> {
        let mut records = self.records.lock().await;
        if matches!(records.get(scope), Some(InMemoryRecord::InProgress { .. })) {
            records.remove(scope);
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub(crate) struct CredentialCommandKvIdempotencyLedger<S> {
    kv: S,
}

impl<S> CredentialCommandKvIdempotencyLedger<S>
where
    S: JetStreamKvEntry + JetStreamKvCreate + JetStreamKeyValueUpdate,
{
    pub(crate) fn new(kv: S) -> Self {
        Self { kv }
    }
}

impl<S> CredentialCommandIdempotencyStore for CredentialCommandKvIdempotencyLedger<S>
where
    S: JetStreamKvEntry + JetStreamKvCreate + JetStreamKeyValueUpdate + JetStreamKeyValueDeleteExpectRevision,
{
    async fn begin(
        &self,
        scope: IdempotencyScope,
        fingerprint: RequestFingerprint,
    ) -> Result<IdempotencyDecision, CredentialCommandIdempotencyStoreError> {
        let key = scope.storage_key();
        for _ in 0..3 {
            let Some(entry) = self
                .kv
                .entry(key.clone())
                .await
                .map_err(CredentialCommandIdempotencyStoreError::backend)?
            else {
                let record = StoredIdempotencyRecord::in_progress(fingerprint.clone());
                match self.kv.create(&key, Bytes::from(encode_record(&record))).await {
                    Ok(_) => return Ok(IdempotencyDecision::Execute),
                    Err(source) if source.kind() == kv::CreateErrorKind::AlreadyExists => continue,
                    Err(source) => return Err(CredentialCommandIdempotencyStoreError::backend(source)),
                }
            };

            if matches!(entry.operation, kv::Operation::Delete | kv::Operation::Purge) {
                let record = StoredIdempotencyRecord::in_progress(fingerprint.clone());
                match self.kv.create(&key, Bytes::from(encode_record(&record))).await {
                    Ok(_) => return Ok(IdempotencyDecision::Execute),
                    Err(source) if source.kind() == kv::CreateErrorKind::AlreadyExists => continue,
                    Err(source) => return Err(CredentialCommandIdempotencyStoreError::backend(source)),
                }
            }

            return decision_from_stored_record(&decode_record(&entry.value)?, &fingerprint);
        }

        Err(CredentialCommandIdempotencyStoreError::Conflict)
    }

    async fn complete(
        &self,
        scope: &IdempotencyScope,
        response: CredentialCommandResponse,
    ) -> Result<(), CredentialCommandIdempotencyStoreError> {
        let key = scope.storage_key();
        let Some(entry) = self
            .kv
            .entry(key.clone())
            .await
            .map_err(CredentialCommandIdempotencyStoreError::backend)?
        else {
            return Err(CredentialCommandIdempotencyStoreError::Conflict);
        };

        if matches!(entry.operation, kv::Operation::Delete | kv::Operation::Purge) {
            return Err(CredentialCommandIdempotencyStoreError::Conflict);
        }

        let record = decode_record(&entry.value)?;
        let fingerprint = record.fingerprint;
        let completed = StoredIdempotencyRecord::completed(fingerprint, response);
        self.kv
            .update(&key, Bytes::from(encode_record(&completed)), entry.revision)
            .await
            .map_err(|source| {
                if source.kind() == kv::UpdateErrorKind::WrongLastRevision {
                    CredentialCommandIdempotencyStoreError::Conflict
                } else {
                    CredentialCommandIdempotencyStoreError::backend(source)
                }
            })?;
        Ok(())
    }

    async fn abandon(&self, scope: &IdempotencyScope) -> Result<(), CredentialCommandIdempotencyStoreError> {
        let key = scope.storage_key();
        let Some(entry) = self
            .kv
            .entry(key.clone())
            .await
            .map_err(CredentialCommandIdempotencyStoreError::backend)?
        else {
            return Ok(());
        };

        if matches!(entry.operation, kv::Operation::Delete | kv::Operation::Purge) {
            return Ok(());
        }

        let record = decode_record(&entry.value)?;
        if matches!(record.status, StoredIdempotencyStatus::InProgress) {
            self.kv
                .delete_expect_revision(&key, Some(entry.revision))
                .await
                .map_err(|source| {
                    if source.kind() == kv::DeleteErrorKind::WrongLastRevision {
                        CredentialCommandIdempotencyStoreError::Conflict
                    } else {
                        CredentialCommandIdempotencyStoreError::backend(source)
                    }
                })?;
        }

        Ok(())
    }
}

struct StoredIdempotencyRecord {
    fingerprint: RequestFingerprint,
    status: StoredIdempotencyStatus,
    response: Option<CredentialCommandResponse>,
}

impl StoredIdempotencyRecord {
    fn in_progress(fingerprint: RequestFingerprint) -> Self {
        Self {
            fingerprint,
            status: StoredIdempotencyStatus::InProgress,
            response: None,
        }
    }

    fn completed(fingerprint: RequestFingerprint, response: CredentialCommandResponse) -> Self {
        Self {
            fingerprint,
            status: StoredIdempotencyStatus::Completed,
            response: Some(response),
        }
    }
}

enum StoredIdempotencyStatus {
    InProgress,
    Completed,
}

pub(crate) async fn open(
    context: jetstream::Context,
) -> Result<CredentialCommandKvIdempotencyLedger<kv::Store>, CredentialManagementIdempotencyOpenError> {
    let store = provision_bucket::<_, kv::Store>(&context).await?;
    Ok(CredentialCommandKvIdempotencyLedger::new(store))
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum CredentialManagementIdempotencyOpenError {
    #[error("failed to create credential management idempotency bucket: {0}")]
    Create(#[source] Box<context::CreateKeyValueError>),
    #[error("failed to open existing credential management idempotency bucket: {0}")]
    OpenExisting(#[source] Box<context::KeyValueError>),
    #[error("failed to inspect credential management idempotency bucket: {0}")]
    Inspect(#[source] kv::StatusError),
    #[error("{source}")]
    Incompatible {
        #[source]
        source: IncompatibleCredentialManagementIdempotencyBucket,
    },
}

#[derive(Debug, thiserror::Error)]
#[error(
    "credential management idempotency bucket is incompatible: expected history {expected_history}, got {actual_history}; expected max age {expected_max_age:?}, got {actual_max_age:?}"
)]
pub(crate) struct IncompatibleCredentialManagementIdempotencyBucket {
    expected_history: i64,
    actual_history: i64,
    expected_max_age: Duration,
    actual_max_age: Duration,
}

pub(crate) async fn provision_bucket<C, S>(client: &C) -> Result<S, CredentialManagementIdempotencyOpenError>
where
    C: JetStreamCreateKeyValue<Store = S> + JetStreamGetKeyValue<Store = S>,
    S: JetStreamKeyValueStatus,
{
    let store = match client.create_key_value(bucket_config()).await {
        Ok(store) => store,
        Err(source) if is_create_key_value_already_exists(&source) => client
            .get_key_value(CREDENTIAL_MANAGEMENT_IDEMPOTENCY_BUCKET)
            .await
            .map_err(|source| CredentialManagementIdempotencyOpenError::OpenExisting(Box::new(source)))?,
        Err(source) => return Err(CredentialManagementIdempotencyOpenError::Create(Box::new(source))),
    };

    validate_bucket(&store).await?;
    Ok(store)
}

pub(crate) fn bucket_config() -> kv::Config {
    kv::Config {
        bucket: CREDENTIAL_MANAGEMENT_IDEMPOTENCY_BUCKET.to_string(),
        history: 1,
        max_age: CREDENTIAL_MANAGEMENT_IDEMPOTENCY_MAX_AGE,
        ..Default::default()
    }
}

async fn validate_bucket<S>(store: &S) -> Result<(), CredentialManagementIdempotencyOpenError>
where
    S: JetStreamKeyValueStatus,
{
    let status = store
        .status()
        .await
        .map_err(CredentialManagementIdempotencyOpenError::Inspect)?;
    let history = status.history();
    let max_age = status.max_age();
    if history != 1 || max_age != CREDENTIAL_MANAGEMENT_IDEMPOTENCY_MAX_AGE {
        return Err(CredentialManagementIdempotencyOpenError::Incompatible {
            source: IncompatibleCredentialManagementIdempotencyBucket {
                expected_history: 1,
                actual_history: history,
                expected_max_age: CREDENTIAL_MANAGEMENT_IDEMPOTENCY_MAX_AGE,
                actual_max_age: max_age,
            },
        });
    }
    Ok(())
}

fn decision_from_stored_record(
    record: &StoredIdempotencyRecord,
    fingerprint: &RequestFingerprint,
) -> Result<IdempotencyDecision, CredentialCommandIdempotencyStoreError> {
    if &record.fingerprint != fingerprint {
        return Ok(IdempotencyDecision::Conflict);
    }

    match record.status {
        StoredIdempotencyStatus::InProgress => Ok(IdempotencyDecision::InProgress),
        StoredIdempotencyStatus::Completed => Ok(IdempotencyDecision::Replay(record.response.clone().ok_or_else(
            || {
                CredentialCommandIdempotencyStoreError::invalid_record(
                    "completed idempotency record is missing response",
                )
            },
        )?)),
    }
}

fn encode_record(record: &StoredIdempotencyRecord) -> Vec<u8> {
    proto_record(record).encode_to_vec()
}

fn decode_record(value: &[u8]) -> Result<StoredIdempotencyRecord, CredentialCommandIdempotencyStoreError> {
    let record = proto::CredentialManagementIdempotencyRecord::decode_from_slice(value)
        .map_err(CredentialCommandIdempotencyStoreError::codec)?;
    let status = decode_status(record.status.as_ref())?;
    let response = record.response.as_option().map(decode_response).transpose()?;

    Ok(StoredIdempotencyRecord {
        fingerprint: RequestFingerprint::new(record.fingerprint),
        status,
        response,
    })
}

fn proto_record(record: &StoredIdempotencyRecord) -> proto::CredentialManagementIdempotencyRecord {
    proto::CredentialManagementIdempotencyRecord {
        fingerprint: record.fingerprint.0.clone(),
        status: Some(proto_status(&record.status).into()),
        response: record
            .response
            .as_ref()
            .map(proto_response)
            .map(MessageField::some)
            .unwrap_or_else(MessageField::none),
    }
}

fn proto_status(status: &StoredIdempotencyStatus) -> proto::CredentialManagementIdempotencyStatus {
    match status {
        StoredIdempotencyStatus::InProgress => {
            proto::CredentialManagementIdempotencyStatus::CREDENTIAL_MANAGEMENT_IDEMPOTENCY_STATUS_IN_PROGRESS
        }
        StoredIdempotencyStatus::Completed => {
            proto::CredentialManagementIdempotencyStatus::CREDENTIAL_MANAGEMENT_IDEMPOTENCY_STATUS_COMPLETED
        }
    }
}

fn decode_status(
    value: Option<&EnumValue<proto::CredentialManagementIdempotencyStatus>>,
) -> Result<StoredIdempotencyStatus, CredentialCommandIdempotencyStoreError> {
    let Some(value) = value else {
        return Err(CredentialCommandIdempotencyStoreError::invalid_record(
            "idempotency record is missing status",
        ));
    };
    let Some(status) = value.as_known() else {
        return Err(CredentialCommandIdempotencyStoreError::invalid_record(
            "idempotency record has unknown status",
        ));
    };

    match status {
        proto::CredentialManagementIdempotencyStatus::CREDENTIAL_MANAGEMENT_IDEMPOTENCY_STATUS_IN_PROGRESS => {
            Ok(StoredIdempotencyStatus::InProgress)
        }
        proto::CredentialManagementIdempotencyStatus::CREDENTIAL_MANAGEMENT_IDEMPOTENCY_STATUS_COMPLETED => {
            Ok(StoredIdempotencyStatus::Completed)
        }
        proto::CredentialManagementIdempotencyStatus::CREDENTIAL_MANAGEMENT_IDEMPOTENCY_STATUS_UNSPECIFIED => Err(
            CredentialCommandIdempotencyStoreError::invalid_record("idempotency record has unspecified status"),
        ),
    }
}

fn proto_response(response: &CredentialCommandResponse) -> proto::CredentialCommandResponse {
    proto::CredentialCommandResponse {
        lifecycle_state: response.lifecycle_state.clone(),
        stream_position: Some(response.stream_position),
        credential_ref: MessageField::some(proto_credential_ref_response(&response.credential_ref)),
    }
}

fn proto_credential_ref_response(response: &CredentialRefResponse) -> proto::CredentialRefResponse {
    proto::CredentialRefResponse {
        id: response.id.clone(),
        version: Some(response.version),
        owner_id: response.owner_id.clone(),
        source: response.source.clone(),
        scope_key: response.scope_key.clone(),
        kind: response.kind.clone(),
        status: response.status.clone(),
    }
}

fn decode_response(
    response: &proto::CredentialCommandResponse,
) -> Result<CredentialCommandResponse, CredentialCommandIdempotencyStoreError> {
    let credential_ref = response
        .credential_ref
        .as_option()
        .ok_or_else(|| {
            CredentialCommandIdempotencyStoreError::invalid_record(
                "completed idempotency response is missing credential ref",
            )
        })
        .and_then(decode_credential_ref_response)?;

    Ok(CredentialCommandResponse {
        lifecycle_state: response.lifecycle_state.clone(),
        stream_position: response.stream_position.ok_or_else(|| {
            CredentialCommandIdempotencyStoreError::invalid_record(
                "completed idempotency response is missing stream position",
            )
        })?,
        credential_ref,
    })
}

fn decode_credential_ref_response(
    response: &proto::CredentialRefResponse,
) -> Result<CredentialRefResponse, CredentialCommandIdempotencyStoreError> {
    Ok(CredentialRefResponse {
        id: response.id.clone(),
        version: response.version.ok_or_else(|| {
            CredentialCommandIdempotencyStoreError::invalid_record(
                "completed idempotency response is missing credential ref version",
            )
        })?,
        owner_id: response.owner_id.clone(),
        source: response.source.clone(),
        scope_key: response.scope_key.clone(),
        kind: response.kind.clone(),
        status: response.status.clone(),
    })
}

fn update_hash(hasher: &mut Sha256, value: &str) {
    hasher.update((value.len() as u64).to_be_bytes());
    hasher.update(value.as_bytes());
}

#[cfg(test)]
mod tests {
    use async_nats::jetstream::context::{CreateKeyValueErrorKind, KeyValueErrorKind};
    use trogon_nats::jetstream::{MockJetStreamKvClient, MockJetStreamKvStore};

    use super::*;
    use crate::credential_management::{CredentialCommandResponse, CredentialRefResponse};

    fn scope(owner: &str, target: &str, key: &str) -> IdempotencyScope {
        IdempotencyScope::new(owner, "credential.create", target, IdempotencyKey::new(key).unwrap())
    }

    fn fingerprint(value: &str) -> RequestFingerprint {
        RequestFingerprint::new(value)
    }

    fn response(version: u64) -> CredentialCommandResponse {
        CredentialCommandResponse {
            lifecycle_state: "active".to_string(),
            stream_position: 2,
            credential_ref: CredentialRefResponse {
                id: "openbao:tenant-1:github/primary:webhook_secret".to_string(),
                version,
                owner_id: "tenant-1".to_string(),
                source: "github".to_string(),
                scope_key: "github/primary".to_string(),
                kind: "webhook_secret".to_string(),
                status: "active".to_string(),
            },
        }
    }

    #[tokio::test]
    async fn kv_ledger_creates_in_progress_then_replays_completed_response() {
        let kv = MockJetStreamKvStore::new();
        kv.enqueue_entry_none();
        kv.enqueue_entry(
            Bytes::from(encode_record(&StoredIdempotencyRecord::in_progress(fingerprint(
                "fp-1",
            )))),
            1,
            kv::Operation::Put,
        );
        kv.enqueue_entry(
            Bytes::from(encode_record(&StoredIdempotencyRecord::completed(
                fingerprint("fp-1"),
                response(1),
            ))),
            2,
            kv::Operation::Put,
        );
        let ledger = CredentialCommandKvIdempotencyLedger::new(kv.clone());
        let scope = scope("tenant-1", "openbao:tenant-1:github/primary:webhook_secret", "retry-1");

        let first = ledger.begin(scope.clone(), fingerprint("fp-1")).await.unwrap();
        ledger.complete(&scope, response(1)).await.unwrap();
        let second = ledger.begin(scope, fingerprint("fp-1")).await.unwrap();

        assert!(matches!(first, IdempotencyDecision::Execute));
        assert!(matches!(second, IdempotencyDecision::Replay(_)));
        assert_eq!(kv.create_calls().len(), 1);
        assert_eq!(kv.update_calls().len(), 1);
    }

    #[tokio::test]
    async fn kv_ledger_conflicts_when_fingerprint_differs() {
        let record = StoredIdempotencyRecord::completed(fingerprint("fp-1"), response(1));
        let kv = MockJetStreamKvStore::new();
        kv.enqueue_entry(Bytes::from(encode_record(&record)), 2, kv::Operation::Put);
        let ledger = CredentialCommandKvIdempotencyLedger::new(kv);

        let decision = ledger
            .begin(
                scope("tenant-1", "openbao:tenant-1:github/primary:webhook_secret", "retry-1"),
                fingerprint("fp-2"),
            )
            .await
            .unwrap();

        assert!(matches!(decision, IdempotencyDecision::Conflict));
    }

    #[tokio::test]
    async fn kv_ledger_abandon_deletes_in_progress_record() {
        let record = StoredIdempotencyRecord::in_progress(fingerprint("fp-1"));
        let kv = MockJetStreamKvStore::new();
        let ledger = CredentialCommandKvIdempotencyLedger::new(kv.clone());
        let scope = scope("tenant-1", "openbao:tenant-1:github/primary:webhook_secret", "retry-1");
        let key = scope.storage_key();
        kv.enqueue_entry(Bytes::from(encode_record(&record)), 7, kv::Operation::Put);

        ledger.abandon(&scope).await.unwrap();

        assert_eq!(kv.delete_calls(), vec![(key, Some(7))]);
    }

    #[tokio::test]
    async fn kv_ledger_abandon_keeps_completed_record() {
        let record = StoredIdempotencyRecord::completed(fingerprint("fp-1"), response(1));
        let kv = MockJetStreamKvStore::new();
        let ledger = CredentialCommandKvIdempotencyLedger::new(kv.clone());
        let scope = scope("tenant-1", "openbao:tenant-1:github/primary:webhook_secret", "retry-1");
        kv.enqueue_entry(Bytes::from(encode_record(&record)), 7, kv::Operation::Put);

        ledger.abandon(&scope).await.unwrap();

        assert!(kv.delete_calls().is_empty());
    }

    #[test]
    fn bucket_config_matches_runtime_contract() {
        let config = bucket_config();

        assert_eq!(config.bucket, CREDENTIAL_MANAGEMENT_IDEMPOTENCY_BUCKET);
        assert_eq!(config.history, 1);
        assert_eq!(config.max_age, CREDENTIAL_MANAGEMENT_IDEMPOTENCY_MAX_AGE);
    }

    #[tokio::test]
    async fn provision_bucket_falls_back_to_get_on_already_exists() {
        let client = MockJetStreamKvClient::new();
        client.fail_create_already_exists();
        let store = MockJetStreamKvStore::new();
        store.set_settings(1, CREDENTIAL_MANAGEMENT_IDEMPOTENCY_MAX_AGE, true, None);
        client.set_get_result(store);

        let result = provision_bucket::<_, MockJetStreamKvStore>(&client).await;

        assert!(result.is_ok());
        assert_eq!(
            client.requested_buckets(),
            vec![CREDENTIAL_MANAGEMENT_IDEMPOTENCY_BUCKET.to_string()]
        );
    }

    #[tokio::test]
    async fn provision_bucket_surfaces_get_errors_after_already_exists() {
        let client = MockJetStreamKvClient::new();
        client.fail_create_already_exists();
        client.fail_get(KeyValueErrorKind::GetBucket);

        let error = provision_bucket::<_, MockJetStreamKvStore>(&client).await.unwrap_err();

        assert!(
            error
                .to_string()
                .contains("failed to open existing credential management idempotency bucket")
        );
    }

    #[tokio::test]
    async fn provision_bucket_surfaces_create_errors() {
        let client = MockJetStreamKvClient::new();
        client.fail_create(CreateKeyValueErrorKind::TimedOut);

        let error = provision_bucket::<_, MockJetStreamKvStore>(&client).await.unwrap_err();

        assert!(
            error
                .to_string()
                .contains("failed to create credential management idempotency bucket")
        );
    }

    #[tokio::test]
    async fn provision_bucket_rejects_incompatible_max_age() {
        let client = MockJetStreamKvClient::new();
        let store = MockJetStreamKvStore::new();
        store.set_settings(1, Duration::ZERO, true, None);
        client.set_create_result(store);

        let error = provision_bucket::<_, MockJetStreamKvStore>(&client).await.unwrap_err();

        assert!(error.to_string().contains("idempotency bucket is incompatible"));
    }
}
