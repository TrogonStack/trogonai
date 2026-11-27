use super::*;

/// Journal in a JetStream KV bucket so a retry that lands on another broker
/// replica still finds the first launch.
pub struct NatsKvCommandLaunchStore {
    kv: kv::Store,
    custody: OnceCell<kv::Store>,
    custody_context: jetstream::Context,
    custody_bucket: String,
    custody_replicas: usize,
}

impl NatsKvCommandLaunchStore {
    pub async fn new(jetstream: jetstream::Context, bucket: &str, replicas: usize) -> Result<Self, CommandLaunchError> {
        let kv = jetstream
            .create_or_update_key_value(kv::Config {
                bucket: bucket.to_string(),
                history: 1,
                max_age: LAUNCH_KEY_MAX_AGE,
                num_replicas: replicas,
                ..Default::default()
            })
            .await
            .map_err(CommandLaunchError::backend)?;
        Ok(Self {
            kv,
            custody: OnceCell::new(),
            custody_context: jetstream,
            custody_bucket: format!("{bucket}-custody"),
            custody_replicas: replicas,
        })
    }

    fn key(lease: &LeaseId, key: &LaunchKey) -> String {
        format!("{}.{key}", lease_token(lease))
    }

    async fn custody_storage(&self) -> Result<&kv::Store, CommandLaunchError> {
        let custody = self
            .custody
            .get_or_try_init(|| async {
                if let Ok(store) = self.custody_context.get_key_value(&self.custody_bucket).await {
                    return Ok::<_, CommandLaunchError>(store);
                }
                match self
                    .custody_context
                    .create_key_value(kv::Config {
                        bucket: self.custody_bucket.clone(),
                        history: 1,
                        max_bytes: 64 * 1024 * 1024,
                        max_value_size: MAX_CANCELLATION_AUTHORITY_BYTES as i32,
                        storage: stream::StorageType::File,
                        num_replicas: self.custody_replicas,
                        ..Default::default()
                    })
                    .await
                {
                    Ok(store) => Ok(store),
                    Err(create) => self
                        .custody_context
                        .get_key_value(&self.custody_bucket)
                        .await
                        .map_err(|load| CommandLaunchError::CustodyUnavailable {
                            create: create.to_string(),
                            load: load.to_string(),
                        }),
                }
            })
            .await?;
        self.check_custody_storage(custody).await?;
        Ok(custody)
    }

    async fn existing_custody_storage(&self) -> Result<Option<&kv::Store>, CommandLaunchError> {
        if self.custody.get().is_none() {
            match self
                .custody_context
                .get_stream(format!("KV_{}", self.custody_bucket))
                .await
            {
                Ok(_) => {}
                Err(error)
                    if matches!(error.kind(), jetstream::context::GetStreamErrorKind::JetStream(ref server)
                    if server.error_code() == jetstream::ErrorCode::STREAM_NOT_FOUND) =>
                {
                    return Ok(None);
                }
                Err(error) => return Err(CommandLaunchError::backend(error)),
            }
            self.custody
                .get_or_try_init(|| async {
                    self.custody_context
                        .get_key_value(&self.custody_bucket)
                        .await
                        .map_err(CommandLaunchError::backend)
                })
                .await?;
        }
        let custody = self.custody.get().ok_or(CommandLaunchError::CustodyMissing)?;
        self.check_custody_storage(custody).await?;
        Ok(Some(custody))
    }

    async fn check_custody_storage(&self, custody: &kv::Store) -> Result<(), CommandLaunchError> {
        let mut native = custody.stream.clone();
        let info = native.info().await.map_err(CommandLaunchError::backend)?;
        if !(matches!(&info.config, stream::Config {
                max_age, discard: stream::DiscardPolicy::New, retention: stream::RetentionPolicy::Limits,
                storage: stream::StorageType::File, allow_message_ttl: false,
                subject_delete_marker_ttl: None, subject_transform: None, republish: None,
                sealed: false, no_ack: false, mirror: None, sources, subjects, ..
            } if max_age.is_zero() && sources.as_ref().is_none_or(Vec::is_empty)
                && subjects == &[format!("$KV.{}.>", self.custody_bucket)]
                && info.config.max_messages_per_subject == 1
                && info.config.max_bytes > 0
                && (1..=MAX_CANCELLATION_AUTHORITY_BYTES as i32).contains(&info.config.max_message_size)))
        {
            return Err(CommandLaunchError::CustodyUnbounded);
        }
        Ok(())
    }
}

/// Lease ids are free-form strings, so they are hashed into a single KV key
/// token rather than trusted to be valid subject characters.
fn lease_token(lease: &LeaseId) -> String {
    hex::encode(sha2::Sha256::digest(lease.as_str().as_bytes()))
}

#[async_trait]
impl CommandLaunchStore for NatsKvCommandLaunchStore {
    fn cancellation_custody_support(&self) -> CancellationCustodySupport {
        CancellationCustodySupport::Retained
    }

    async fn claim(&self, lease: &LeaseId, key: &LaunchKey, fingerprint: &str) -> Result<Claim, CommandLaunchError> {
        let pending = CommandLaunch::new(fingerprint, LaunchState::Pending);
        let storage = Self::key(lease, key);
        for _ in 0..8 {
            match self.kv.create(&storage, serde_json::to_vec(&pending)?.into()).await {
                Ok(_) => return Ok(Claim::Claimed),
                Err(error) if error.kind() == kv::CreateErrorKind::AlreadyExists => {}
                Err(error) => return Err(CommandLaunchError::backend(error)),
            }
            // A concurrent forget can delete the entry between the failed
            // create and this read; claim again in that case.
            if let Some(entry) = self.kv.entry(&storage).await.map_err(CommandLaunchError::backend)?
                && !matches!(entry.operation, kv::Operation::Delete | kv::Operation::Purge)
            {
                return Ok(Claim::Existing(serde_json::from_slice(&entry.value)?));
            }
        }
        Err(CommandLaunchError::ClaimRace)
    }

    async fn retain_cancellation_authority(
        &self,
        lease: &LeaseId,
        key: &LaunchKey,
        authority: &CommandCancellationAuthority,
    ) -> Result<CancellationAuthorityClaim, CommandLaunchError> {
        authority.validate(lease, key)?;
        let custody = self.custody_storage().await?;
        if let Some(existing) = self.cancellation_authority(lease, key).await? {
            return Ok(CancellationAuthorityClaim::Existing(existing));
        }
        let entry = self
            .kv
            .entry(Self::key(lease, key))
            .await
            .map_err(CommandLaunchError::backend)?
            .ok_or(CommandLaunchError::LaunchUnavailable)?;
        if entry.operation != kv::Operation::Put {
            return Err(CommandLaunchError::LaunchUnavailable);
        }
        let launch: CommandLaunch = serde_json::from_slice(&entry.value)?;
        if !(launch.fingerprint == authority.fingerprint && matches!(launch.state, LaunchState::Pending)) {
            return Err(CommandLaunchError::AuthorityMismatch);
        }
        let storage = Self::key(lease, key);
        match custody.create(&storage, serde_json::to_vec(authority)?.into()).await {
            Ok(_) => Ok(CancellationAuthorityClaim::New),
            Err(error) if error.kind() == kv::CreateErrorKind::AlreadyExists => {
                let existing = self
                    .cancellation_authority(lease, key)
                    .await?
                    .ok_or(CommandLaunchError::AuthorityDisappeared)?;
                Ok(CancellationAuthorityClaim::Existing(existing))
            }
            Err(error) => Err(CommandLaunchError::backend(error)),
        }
    }

    async fn cancellation_authority(
        &self,
        lease: &LeaseId,
        key: &LaunchKey,
    ) -> Result<Option<CommandCancellationAuthority>, CommandLaunchError> {
        let Some(custody) = self.existing_custody_storage().await? else {
            return Ok(None);
        };
        let Some(entry) = custody
            .entry(Self::key(lease, key))
            .await
            .map_err(CommandLaunchError::backend)?
        else {
            return Ok(None);
        };
        if !(entry.operation == kv::Operation::Put && entry.value.len() <= MAX_CANCELLATION_AUTHORITY_BYTES) {
            return Err(CommandLaunchError::AuthorityUnreadable);
        }
        let authority: CommandCancellationAuthority = serde_json::from_slice(&entry.value)?;
        authority.validate(lease, key)?;
        Ok(Some(authority))
    }

    async fn finish(&self, lease: &LeaseId, key: &LaunchKey, launch: &CommandLaunch) -> Result<(), CommandLaunchError> {
        self.kv
            .put(Self::key(lease, key), serde_json::to_vec(launch)?.into())
            .await
            .map_err(CommandLaunchError::backend)?;
        Ok(())
    }

    async fn forget(&self, lease: &LeaseId, key: &LaunchKey) -> Result<(), CommandLaunchError> {
        self.kv
            .purge(Self::key(lease, key))
            .await
            .map_err(CommandLaunchError::backend)?;
        Ok(())
    }

    async fn replace(
        &self,
        lease: &LeaseId,
        key: &LaunchKey,
        expected: &CommandLaunch,
        next: &CommandLaunch,
    ) -> Result<bool, CommandLaunchError> {
        let storage = Self::key(lease, key);
        let Some(entry) = self.kv.entry(&storage).await.map_err(CommandLaunchError::backend)? else {
            return Ok(false);
        };
        if entry.operation != kv::Operation::Put || serde_json::from_slice::<CommandLaunch>(&entry.value)? != *expected
        {
            return Ok(false);
        }
        match self
            .kv
            .update(&storage, serde_json::to_vec(next)?.into(), entry.revision)
            .await
        {
            Ok(_) => Ok(true),
            Err(error) if error.kind() == kv::UpdateErrorKind::WrongLastRevision => Ok(false),
            Err(error) => Err(CommandLaunchError::backend(error)),
        }
    }

    async fn forget_lease(&self, lease: &LeaseId) -> Result<(), CommandLaunchError> {
        let subject = format!("{}{}.>", self.kv.prefix, lease_token(lease));
        self.kv
            .stream
            .purge()
            .filter(subject)
            .await
            .map_err(CommandLaunchError::backend)?;
        Ok(())
    }
}
