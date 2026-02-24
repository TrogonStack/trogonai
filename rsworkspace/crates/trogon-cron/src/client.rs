use async_nats::jetstream;
use async_nats::jetstream::kv::Operation;
use futures::StreamExt;

use crate::{
    config::JobConfig,
    error::CronError,
    kv::{CONFIG_BUCKET, JOBS_KEY_PREFIX, get_or_create_config_bucket},
};

/// Client for registering and managing CRON job configs in NATS KV.
///
/// Multiple processes can use `CronClient` simultaneously — changes are picked up
/// by all running `Scheduler` instances in real time via KV watch.
///
/// # Example
///
/// ```rust,no_run
/// use trogon_cron::{CronClient, JobConfig, Schedule, Action};
///
/// # async fn example() -> Result<(), trogon_cron::CronError> {
/// let nats = async_nats::connect("nats://localhost:4222").await.unwrap();
/// let client = CronClient::new(nats).await?;
///
/// client.register_job(&JobConfig {
///     id: "health".to_string(),
///     schedule: Schedule::Interval { interval_sec: 30 },
///     action: Action::Publish { subject: "cron.health".to_string() },
///     enabled: true,
///     payload: None,
///     retry: None,
/// }).await?;
/// # Ok(())
/// # }
/// ```
pub struct CronClient {
    js: jetstream::Context,
}

impl CronClient {
    /// Connect the client and ensure the config KV bucket exists.
    pub async fn new(nats: async_nats::Client) -> Result<Self, CronError> {
        let js = jetstream::new(nats);
        get_or_create_config_bucket(&js).await?;
        Ok(Self { js })
    }

    /// Register or update a job. Existing jobs with the same `id` are overwritten.
    ///
    /// Structural constraints on `Action::Spawn` configs are validated here so
    /// callers get an immediate error rather than a silent scheduler-side skip.
    /// Filesystem checks (file exists, is executable) are intentionally omitted
    /// because the client may run on a different host than the scheduler.
    pub async fn register_job(&self, config: &JobConfig) -> Result<(), CronError> {
        if let crate::config::Action::Publish { subject } = &config.action {
            if !subject.starts_with("cron.") {
                return Err(CronError::InvalidJobConfig {
                    reason: format!(
                        "publish subject must start with 'cron.', got: {subject}"
                    ),
                });
            }
        }
        if let crate::config::Action::Spawn { bin, args, timeout_sec, .. } = &config.action {
            if !std::path::Path::new(bin).is_absolute() {
                return Err(CronError::InvalidJobConfig {
                    reason: format!("bin must be an absolute path, got: {bin}"),
                });
            }
            for arg in args {
                if arg.contains('\0') {
                    return Err(CronError::InvalidJobConfig {
                        reason: format!("argument contains null byte: {arg:?}"),
                    });
                }
            }
            if timeout_sec.is_some_and(|s| s == 0) {
                return Err(CronError::InvalidJobConfig {
                    reason: "timeout_sec must be >= 1 when set".into(),
                });
            }
        }

        let kv = self
            .js
            .get_key_value(CONFIG_BUCKET)
            .await
            .map_err(|e| CronError::Kv(e.to_string()))?;

        let key = format!("{}{}", JOBS_KEY_PREFIX, config.id);
        let value = serde_json::to_vec(config)?;
        kv.put(key, value.into())
            .await
            .map_err(|e| CronError::Kv(e.to_string()))?;

        tracing::info!(job_id = %config.id, "Job registered");
        Ok(())
    }

    /// Remove a job by id. No-op if the job doesn't exist.
    pub async fn remove_job(&self, id: &str) -> Result<(), CronError> {
        let kv = self
            .js
            .get_key_value(CONFIG_BUCKET)
            .await
            .map_err(|e| CronError::Kv(e.to_string()))?;

        let key = format!("{}{}", JOBS_KEY_PREFIX, id);
        kv.delete(key)
            .await
            .map_err(|e| CronError::Kv(e.to_string()))?;

        tracing::info!(job_id = %id, "Job removed");
        Ok(())
    }

    /// Enable or disable a job without removing it.
    pub async fn set_enabled(&self, id: &str, enabled: bool) -> Result<(), CronError> {
        let mut config = self
            .get_job(id)
            .await?
            .ok_or_else(|| CronError::Kv(format!("job '{}' not found", id)))?;

        config.enabled = enabled;
        self.register_job(&config).await
    }

    /// Get a single job config by id. Returns `None` if not found or deleted.
    pub async fn get_job(&self, id: &str) -> Result<Option<JobConfig>, CronError> {
        let kv = self
            .js
            .get_key_value(CONFIG_BUCKET)
            .await
            .map_err(|e| CronError::Kv(e.to_string()))?;

        let key = format!("{}{}", JOBS_KEY_PREFIX, id);
        match kv
            .entry(key)
            .await
            .map_err(|e| CronError::Kv(e.to_string()))?
        {
            Some(entry) if entry.operation == Operation::Put => {
                let config = serde_json::from_slice(&entry.value)?;
                Ok(Some(config))
            }
            _ => Ok(None),
        }
    }

    /// List all currently active job configs.
    pub async fn list_jobs(&self) -> Result<Vec<JobConfig>, CronError> {
        let kv = self
            .js
            .get_key_value(CONFIG_BUCKET)
            .await
            .map_err(|e| CronError::Kv(e.to_string()))?;

        let mut keys = kv
            .keys()
            .await
            .map_err(|e| CronError::Kv(e.to_string()))?;

        let mut jobs = Vec::new();
        while let Some(result) = keys.next().await {
            let key = result.map_err(|e| CronError::Kv(e.to_string()))?;
            if let Some(value) = kv.get(&key).await.map_err(|e| CronError::Kv(e.to_string()))? {
                if let Ok(config) = serde_json::from_slice::<JobConfig>(&value) {
                    jobs.push(config);
                }
            }
        }
        Ok(jobs)
    }
}
