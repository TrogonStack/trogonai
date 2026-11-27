use std::sync::Arc;
use std::time::Duration;

use tracing::{info, warn};

use super::build_template::build_template;
use super::constants::{DEFAULT_POLL_INTERVAL, FAILURE_TTL, MICROVM_MIN_MEMORY_MIB};
use super::now_secs::now_secs;
use super::{TemplateError, TemplateIndex, TemplateKey, TemplatePhase, TemplateRecord};
use crate::ate_client::ResourceRevision;
use crate::ate_client::{AteError, Control};
use crate::config::{SandboxClass, SubstrateConfig};
use crate::proto;
use crate::resources::validate_image_digest;

pub struct TemplateRegistry {
    index: Arc<dyn TemplateIndex>,
    poll_interval: Duration,
}

impl TemplateRegistry {
    pub fn new(index: Arc<dyn TemplateIndex>) -> Self {
        Self {
            index,
            poll_interval: DEFAULT_POLL_INTERVAL,
        }
    }

    pub fn with_poll_interval(mut self, interval: Duration) -> Self {
        self.poll_interval = interval;
        self
    }

    /// Returns a ref to an `ActorTemplate` whose golden snapshot is ready for this workload,
    /// building one if nobody has.
    ///
    /// Blocking is intended here, and the caller reports it as `SandboxPhase::PullingImage`
    /// rather than folding it into `Allocate`: on a cold image this waits for a golden
    /// snapshot build that takes tens of seconds, and a timeline that hid that would be
    /// lying about where the latency went.
    pub async fn ensure_ready(
        &self,
        control: &dyn Control,
        config: &SubstrateConfig,
        atespace: &str,
        key: &TemplateKey,
        deadline: Duration,
    ) -> Result<proto::ObjectRef, TemplateError> {
        config.validate().map_err(|error| TemplateError::Backend {
            message: error.to_string(),
        })?;
        validate_image_digest(key.image_reference()).map_err(|error| TemplateError::Backend {
            message: error.to_string(),
        })?;
        if key.sandbox_class == SandboxClass::MicroVm && key.resources.memory_mib() < MICROVM_MIN_MEMORY_MIB {
            return Err(TemplateError::Backend {
                message: format!("microvm actors require at least {MICROVM_MIN_MEMORY_MIB} MiB"),
            });
        }
        let kv_key = key.kv_key(atespace);
        let template_ref = proto::ObjectRef {
            atespace: atespace.to_string(),
            name: key.template_name(),
        };

        if let Some(record) = self.lookup(&kv_key, &template_ref).await? {
            match record.phase {
                TemplatePhase::Ready | TemplatePhase::Building => {
                    return self.await_ready(control, key, template_ref, deadline, &kv_key).await;
                }
                TemplatePhase::Failed => {
                    return Err(TemplateError::NotReady {
                        image: key.image_reference().to_string(),
                        reason: record
                            .message
                            .unwrap_or_else(|| "golden snapshot build failed".to_string()),
                    });
                }
            }
        }

        let claimed = self
            .index
            .create(
                &kv_key,
                &TemplateRecord {
                    atespace: atespace.to_string(),
                    name: key.template_name(),
                    phase: TemplatePhase::Building,
                    created_at: now_secs(),
                    message: None,
                },
            )
            .await
            .unwrap_or_else(|e| {
                // A KV fault must not stop the lease: `CreateActorTemplate` is itself
                // idempotent on a content-addressed name, so the worst case without the
                // index is a duplicate create that Substrate collapses to ALREADY_EXISTS.
                warn!(error = %e, key = %kv_key, "template index unavailable, proceeding without the build claim");
                true
            });

        if claimed {
            info!(key = %key.as_str(), image = %key.image_reference(), "building substrate actor template");
            if let Err(e) = self.create_template(control, config, atespace, key).await {
                self.record_failure(&kv_key, atespace, key, &e).await;
                return Err(e);
            }
        }

        self.await_ready(control, key, template_ref, deadline, &kv_key).await
    }

    async fn lookup(
        &self,
        kv_key: &str,
        template_ref: &proto::ObjectRef,
    ) -> Result<Option<TemplateRecord>, TemplateError> {
        match self.index.get(kv_key).await {
            Ok(Some(record)) => {
                if record.atespace != template_ref.atespace || record.name != template_ref.name {
                    return Err(TemplateError::Backend {
                        message: "template cache address differs from the requested identity".into(),
                    });
                }
                if record.phase == TemplatePhase::Failed
                    && now_secs().saturating_sub(record.created_at) >= FAILURE_TTL.as_secs()
                {
                    // Expired failure: drop it so the next lease gets a real retry rather
                    // than inheriting an old verdict.
                    let _ = self.index.delete(kv_key).await;
                    return Ok(None);
                }
                Ok(Some(record))
            }
            Ok(None) => Ok(None),
            Err(e) => {
                warn!(error = %e, key = %kv_key, "template index lookup failed");
                Ok(None)
            }
        }
    }

    async fn create_template(
        &self,
        control: &dyn Control,
        config: &SubstrateConfig,
        atespace: &str,
        key: &TemplateKey,
    ) -> Result<(), TemplateError> {
        let request = build_template(config, atespace, key);
        match control.create_actor_template(request.clone()).await {
            Ok(_) => Ok(()),
            Err(AteError::AlreadyExists { .. }) => {
                self.recreate_if_permanently_failed(control, atespace, key, request)
                    .await
            }
            Err(e) => Err(e.into()),
        }
    }

    /// `CreateActorTemplate` collapsing to `AlreadyExists` on a content-addressed name usually
    /// means another worker already built this template, and the right move is to fall
    /// through to `await_ready`. But Substrate never retries a golden build on its own once it
    /// has permanently failed, so an existing template whose golden snapshot already carries
    /// an error is a dead end that `AlreadyExists` alone cannot distinguish from "someone else
    /// is building it right now". Without this, `FAILURE_TTL` expiring locally would never get
    /// a lease a real retry: the local claim would succeed, `CreateActorTemplate` would
    /// collapse to `AlreadyExists` again, and `await_ready` would just observe the same stale
    /// failure forever.
    async fn recreate_if_permanently_failed(
        &self,
        control: &dyn Control,
        atespace: &str,
        key: &TemplateKey,
        request: proto::ActorTemplate,
    ) -> Result<(), TemplateError> {
        let template_ref = proto::ObjectRef {
            atespace: atespace.to_string(),
            name: key.template_name(),
        };
        let existing = match control.get_actor_template(template_ref.clone()).await {
            Ok(template) => template,
            // Cannot tell what state it's in; let `await_ready`'s own retry loop sort it out.
            Err(_) => return Ok(()),
        };
        let golden = existing
            .status
            .unwrap_or_default()
            .golden_snapshot_status
            .unwrap_or_default();
        if golden.golden_tag.is_some() || golden.error_message.is_empty() {
            return Ok(());
        }

        let template_key = key.as_str();
        let reason = &golden.error_message;
        warn!(key = %template_key, %reason, "existing template already failed permanently, deleting and rebuilding");
        if existing
            .metadata
            .as_ref()
            .is_none_or(|meta| meta.atespace != atespace || meta.name != key.template_name())
        {
            return Err(TemplateError::Backend {
                message: "failed template address does not match the requested identity".into(),
            });
        }
        let revision = ResourceRevision::from_metadata(existing.metadata.as_ref())?;
        match control.delete_actor_template(revision).await {
            Ok(()) | Err(AteError::NotFound { .. }) => {}
            Err(e) => return Err(e.into()),
        }
        match control.create_actor_template(request).await {
            Ok(_) | Err(AteError::AlreadyExists { .. }) => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    async fn await_ready(
        &self,
        control: &dyn Control,
        key: &TemplateKey,
        template_ref: proto::ObjectRef,
        deadline: Duration,
        kv_key: &str,
    ) -> Result<proto::ObjectRef, TemplateError> {
        let started = tokio::time::Instant::now();

        loop {
            match control.get_actor_template(template_ref.clone()).await {
                Ok(template) => {
                    if template.metadata.as_ref().is_none_or(|metadata| {
                        metadata.atespace != template_ref.atespace || metadata.name != template_ref.name
                    }) {
                        return Err(TemplateError::Backend {
                            message: "native template address differs from the requested identity".into(),
                        });
                    }
                    let golden = template
                        .status
                        .unwrap_or_default()
                        .golden_snapshot_status
                        .unwrap_or_default();
                    if !golden.error_message.is_empty() {
                        let error = TemplateError::NotReady {
                            image: key.image_reference().to_string(),
                            reason: golden.error_message,
                        };
                        self.record_failure_at(kv_key, &template_ref, &error).await;
                        return Err(error);
                    }
                    if golden.golden_tag.is_some() {
                        self.record_ready(kv_key, &template_ref).await;
                        return Ok(template_ref);
                    }
                }
                // The template we just created may not be visible yet, and a build in
                // progress is exactly what NOT_FOUND and a retryable FAILED_PRECONDITION mean here.
                Err(AteError::NotFound { .. }) => {}
                Err(e) if e.is_retryable() => {}
                Err(e) => return Err(e.into()),
            }

            if started.elapsed() >= deadline {
                let error = TemplateError::NotReady {
                    image: key.image_reference().to_string(),
                    reason: format!("golden snapshot build did not finish within {}s", deadline.as_secs()),
                };
                // Deliberately not recorded as a cached failure: a slow build is still a
                // build, and poisoning the key would make the next lease fail instantly on
                // a template that is about to become ready.
                return Err(error);
            }

            tokio::time::sleep(self.poll_interval).await;
        }
    }

    async fn record_ready(&self, kv_key: &str, template_ref: &proto::ObjectRef) {
        let record = TemplateRecord {
            atespace: template_ref.atespace.clone(),
            name: template_ref.name.clone(),
            phase: TemplatePhase::Ready,
            created_at: now_secs(),
            message: None,
        };
        if let Err(e) = self.index.put(kv_key, &record).await {
            warn!(error = %e, key = %kv_key, "failed to record template as ready");
        }
    }

    async fn record_failure(&self, kv_key: &str, atespace: &str, key: &TemplateKey, error: &TemplateError) {
        let template_ref = proto::ObjectRef {
            atespace: atespace.to_string(),
            name: key.template_name(),
        };
        self.record_failure_at(kv_key, &template_ref, error).await;
    }

    async fn record_failure_at(&self, kv_key: &str, template_ref: &proto::ObjectRef, error: &TemplateError) {
        let record = TemplateRecord {
            atespace: template_ref.atespace.clone(),
            name: template_ref.name.clone(),
            phase: TemplatePhase::Failed,
            created_at: now_secs(),
            message: Some(error.to_string()),
        };
        if let Err(e) = self.index.put(kv_key, &record).await {
            warn!(error = %e, key = %kv_key, "failed to record template failure");
        }
    }
}
