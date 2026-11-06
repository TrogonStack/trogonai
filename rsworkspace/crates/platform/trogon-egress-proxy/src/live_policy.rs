use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_nats::connection::State as ConnectionState;
use async_nats::jetstream::kv::{Operation, Store};
use async_nats::jetstream::message::StreamMessage;
use async_nats::jetstream::stream::Info;
use tokio::sync::watch;
use tokio::time::{Instant, MissedTickBehavior, interval, timeout};
use tokio_util::sync::CancellationToken;

use crate::agent_id::AgentId;
use crate::constants::{
    KV_OPERATION_HEADER, LIVE_POLICY_MAX_AGENTS, LIVE_POLICY_MAX_BYTES, LIVE_POLICY_MAX_LIFETIME,
    LIVE_POLICY_MAX_PERMITS, LIVE_POLICY_MAX_STORE_BYTES, LIVE_POLICY_POLL_INTERVAL, LIVE_POLICY_REQUEST_TIMEOUT,
};
use crate::destination::Destination;
pub use crate::live_policy_config::{KvPolicyConfig, PolicyConfigError, PolicyTransportError};
pub use crate::live_policy_error::LivePolicyError;
use crate::live_policy_wire::Envelope;
use crate::policy_revision::PolicyRevision;
use crate::policy_snapshot::{PolicyObservation, PolicySnapshot, PolicyStatistics, SnapshotVerdict};
use crate::policy_source::PolicySource;
use crate::static_snapshot::StaticSnapshot;

#[derive(PartialEq, Eq)]
struct ConnectionIdentity {
    server: String,
    client: u64,
}

struct ActivePolicy {
    snapshot: StaticSnapshot,
    revision: PolicyRevision,
    issued_at: SystemTime,
    valid_until: SystemTime,
    deadline: Instant,
}

struct AcceptedState {
    active: Option<ActivePolicy>,
    broker_revision: u64,
    generation: u64,
    issued_at: u64,
    document: Vec<u8>,
    connection: ConnectionIdentity,
}

pub struct KvSnapshot {
    source: PolicySource,
    config: KvPolicyConfig,
    client: async_nats::Client,
    state: Mutex<AcceptedState>,
    changed: watch::Sender<()>,
}

pub struct KvPolicyDriver {
    snapshot: Arc<KvSnapshot>,
    store: Store,
    created: async_nats::datetime::DateTime,
}

impl ConnectionIdentity {
    fn current(client: &async_nats::Client) -> Result<Self, LivePolicyError> {
        if client.connection_state() != ConnectionState::Connected {
            return Err(LivePolicyError::Disconnected);
        }
        let info = client.server_info();
        if info.server_id.is_empty() || info.client_id == 0 {
            return Err(LivePolicyError::Disconnected);
        }
        Ok(Self {
            server: info.server_id,
            client: info.client_id,
        })
    }
}

impl KvSnapshot {
    /// The caller must supply a server- and client-authenticated NATS connection
    /// with read-only access to this exact scoped authority. An opaque Client
    /// does not expose evidence that this constructor can authenticate.
    pub async fn open(
        client: async_nats::Client,
        config: KvPolicyConfig,
    ) -> Result<(Arc<Self>, KvPolicyDriver), LivePolicyError> {
        timeout(LIVE_POLICY_REQUEST_TIMEOUT, client.flush())
            .await
            .map_err(|_| LivePolicyError::Timeout)?
            .map_err(|_| LivePolicyError::Disconnected)?;
        let connection = ConnectionIdentity::current(&client)?;
        let store = timeout(
            LIVE_POLICY_REQUEST_TIMEOUT,
            async_nats::jetstream::new(client.clone()).get_key_value(&config.bucket),
        )
        .await
        .map_err(|_| LivePolicyError::Timeout)?
        .map_err(LivePolicyError::Bucket)?;
        let info = timeout(LIVE_POLICY_REQUEST_TIMEOUT, store.stream.get_info())
            .await
            .map_err(|_| LivePolicyError::Timeout)?
            .map_err(LivePolicyError::Information)?;
        validate_bucket(&info)?;
        let created = info.created;
        let (changed, _) = watch::channel(());
        let snapshot = Arc::new(Self {
            source: PolicySource::NatsKv {
                bucket: config.bucket.clone(),
                incarnation: config.incarnation,
            },
            config,
            client,
            changed,
            state: Mutex::new(AcceptedState {
                active: None,
                broker_revision: 0,
                generation: 0,
                issued_at: 0,
                document: Vec::new(),
                connection,
            }),
        });
        let driver = KvPolicyDriver {
            snapshot: Arc::clone(&snapshot),
            store,
            created,
        };
        driver.refresh().await?;
        Ok((snapshot, driver))
    }

    fn state(&self) -> Result<MutexGuard<'_, AcceptedState>, LivePolicyError> {
        self.state.lock().map_err(|_| LivePolicyError::State)
    }

    pub fn statistics(&self) -> Option<PolicyStatistics> {
        self.current_state(SystemTime::now()).ok().and_then(|state| {
            state.active.as_ref().map(|active| PolicyStatistics {
                agents: active.snapshot.agent_count(),
                permits: active.snapshot.permit_count(),
            })
        })
    }

    fn current_state(&self, now: SystemTime) -> Result<MutexGuard<'_, AcceptedState>, LivePolicyError> {
        let mut state = self.state()?;
        let connection = ConnectionIdentity::current(&self.client);
        match connection {
            Ok(current) if current == state.connection => {}
            Ok(current) => {
                if state.active.take().is_some() {
                    self.changed.send_replace(());
                }
                state.connection = current;
            }
            Err(error) => {
                if state.active.take().is_some() {
                    self.changed.send_replace(());
                }
                return Err(error);
            }
        }
        if state.active.as_ref().is_some_and(|active| {
            now < active.issued_at || now >= active.valid_until || Instant::now() >= active.deadline
        }) && state.active.take().is_some()
        {
            self.changed.send_replace(());
        }
        Ok(state)
    }

    fn withdraw(&self) {
        let changed = match self.state() {
            Ok(mut state) => state.active.take().is_some(),
            Err(_) => true,
        };
        if changed {
            self.changed.send_replace(());
        }
    }

    fn apply(&self, message: StreamMessage) -> Result<(), LivePolicyError> {
        let observed_at = Instant::now();
        let now = SystemTime::now();
        let mut state = self.current_state(now)?;
        let before = state.active.as_ref().map(|active| (active.revision, active.deadline));
        let result = self.apply_to_state(&mut state, message, observed_at, now);
        if result.is_err() {
            state.active = None;
        }
        let after = state.active.as_ref().map(|active| (active.revision, active.deadline));
        if before != after {
            self.changed.send_replace(());
        }
        result
    }

    fn apply_to_state(
        &self,
        state: &mut AcceptedState,
        message: StreamMessage,
        observed_at: Instant,
        now: SystemTime,
    ) -> Result<(), LivePolicyError> {
        if message.sequence < state.broker_revision {
            state.active = None;
            return Err(LivePolicyError::Regression);
        }
        if message.sequence == state.broker_revision && message.payload.as_ref() != state.document {
            state.active = None;
            return Err(LivePolicyError::Regression);
        }
        state.broker_revision = message.sequence;
        let operation = message
            .headers
            .get(KV_OPERATION_HEADER)
            .map(|value| value.as_str().parse::<Operation>())
            .transpose()
            .map_err(|_| LivePolicyError::Removed)?;
        let subject = format!("$KV.{}.{}", self.config.bucket, self.config.key());
        if message.subject.as_str() != subject || operation.is_some_and(|op| op != Operation::Put) {
            state.active = None;
            return Err(LivePolicyError::Removed);
        }
        if message.payload.len() > LIVE_POLICY_MAX_BYTES {
            return Err(LivePolicyError::Bounds);
        }
        let envelope: Envelope = serde_json::from_slice(&message.payload).map_err(|_| LivePolicyError::Envelope)?;
        if envelope.tenant != self.config.tenant.as_str() || envelope.project != self.config.project.as_str() {
            return Err(LivePolicyError::Scope);
        }
        if envelope.incarnation != self.config.incarnation {
            return Err(LivePolicyError::Incarnation);
        }
        let issued_at = UNIX_EPOCH
            .checked_add(Duration::from_secs(envelope.issued_at))
            .ok_or(LivePolicyError::Inactive)?;
        let valid_until = UNIX_EPOCH
            .checked_add(Duration::from_secs(envelope.valid_until))
            .ok_or(LivePolicyError::Inactive)?;
        let lifetime = valid_until
            .duration_since(issued_at)
            .map_err(|_| LivePolicyError::Lease)?;
        if lifetime.is_zero() || lifetime > LIVE_POLICY_MAX_LIFETIME {
            return Err(LivePolicyError::Lease);
        }
        if now < issued_at || now >= valid_until {
            return Err(LivePolicyError::Inactive);
        }
        if envelope.generation.get() == state.generation && message.payload.as_ref() == state.document {
            return if state.active.is_some() {
                Ok(())
            } else {
                Err(LivePolicyError::Regression)
            };
        }
        if envelope.generation.get() <= state.generation || envelope.issued_at < state.issued_at {
            return Err(LivePolicyError::Regression);
        }
        let policy = StaticSnapshot::from_live_toml(self.source.clone(), &envelope.policy)
            .map_err(|_| LivePolicyError::Policy)?;
        if policy.agent_count() > LIVE_POLICY_MAX_AGENTS || policy.permit_count() > LIVE_POLICY_MAX_PERMITS {
            return Err(LivePolicyError::Bounds);
        }
        let remaining = valid_until.duration_since(now).map_err(|_| LivePolicyError::Inactive)?;
        let deadline = observed_at + remaining;
        if SystemTime::now() >= valid_until || Instant::now() >= deadline {
            return Err(LivePolicyError::Inactive);
        }
        state.active = Some(ActivePolicy {
            snapshot: policy,
            revision: PolicyRevision {
                incarnation: envelope.incarnation,
                generation: envelope.generation,
            },
            issued_at,
            valid_until,
            deadline,
        });
        state.generation = envelope.generation.get();
        state.issued_at = envelope.issued_at;
        state.document = message.payload.to_vec();
        Ok(())
    }
}

impl PolicySnapshot for KvSnapshot {
    fn subscribe(&self) -> Option<watch::Receiver<()>> {
        Some(self.changed.subscribe())
    }
    fn source(&self) -> &PolicySource {
        &self.source
    }
    fn lookup(&self, agent: &AgentId, destination: &Destination, now: SystemTime) -> SnapshotVerdict {
        self.observe(agent, destination, now).verdict
    }

    fn observe(&self, agent: &AgentId, destination: &Destination, now: SystemTime) -> PolicyObservation {
        match self.current_state(now).ok().and_then(|state| {
            state.active.as_ref().map(|active| PolicyObservation {
                verdict: active.snapshot.lookup(agent, destination, now),
                revision: Some(active.revision),
                deadline: Some(active.deadline),
            })
        }) {
            Some(observation) => observation,
            None => PolicyObservation {
                verdict: SnapshotVerdict::Unavailable,
                revision: None,
                deadline: None,
            },
        }
    }

    fn revision_is_current(&self, revision: Option<PolicyRevision>, now: SystemTime) -> bool {
        self.current_state(now).ok().is_some_and(|state| {
            state
                .active
                .as_ref()
                .is_some_and(|active| Some(active.revision) == revision)
        })
    }
}

impl KvPolicyDriver {
    async fn refresh(&self) -> Result<(), LivePolicyError> {
        let result = timeout(LIVE_POLICY_REQUEST_TIMEOUT, async {
            drop(self.snapshot.current_state(SystemTime::now())?);
            let info = self
                .store
                .stream
                .get_info()
                .await
                .map_err(LivePolicyError::Information)?;
            validate_bucket(&info)?;
            if info.created != self.created {
                return Err(LivePolicyError::StreamIncarnation);
            }
            let subject = format!("$KV.{}.{}", self.snapshot.config.bucket, self.snapshot.config.key());
            let message = self
                .store
                .stream
                .get_last_raw_message_by_subject(&subject)
                .await
                .map_err(LivePolicyError::Entry)?;
            self.snapshot.apply(message)
        })
        .await
        .map_err(|_| LivePolicyError::Timeout)
        .and_then(|result| result);
        if result.is_err() {
            self.snapshot.withdraw();
        }
        result
    }

    pub async fn run(self, shutdown: CancellationToken) {
        let mut ticks = interval(LIVE_POLICY_POLL_INTERVAL);
        ticks.set_missed_tick_behavior(MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                biased;
                () = shutdown.cancelled() => break,
                _ = ticks.tick() => {
                    if let Err(error) = self.refresh().await {
                        tracing::warn!(reason = error.reason(), "live egress policy unavailable");
                    }
                }
            }
        }
    }
}

impl Drop for KvPolicyDriver {
    fn drop(&mut self) {
        self.snapshot.withdraw();
    }
}

fn validate_bucket(info: &Info) -> Result<(), LivePolicyError> {
    let config = &info.config;
    if config.max_message_size <= 0
        || config.max_message_size as usize > LIVE_POLICY_MAX_BYTES
        || config.max_bytes <= 0
        || config.max_bytes > LIVE_POLICY_MAX_STORE_BYTES
        || config.max_messages_per_subject != 1
        || config.mirror.is_some()
        || config.sources.as_ref().is_some_and(|sources| !sources.is_empty())
        || config.max_age.is_zero()
        || config.max_age > LIVE_POLICY_MAX_LIFETIME
    {
        return Err(LivePolicyError::BucketConfiguration);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
