use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

use kube::Client;
use trogon_sandbox_contract::IsolationTier;
use trogon_sandbox_protocol::LifecycleEmitter;

use crate::k8s_ops::constants::{IMAGE_USER_CACHE_TTL, IMAGE_USER_LOOKUP_TIMEOUT};
use crate::k8s_ops::{RealK8sOps, image_user_is_non_root_uid};
use crate::{DEFAULT_AGENT_UID, ImageUserResolver, K8sMicroVmConfig, K8sOps, K8sProviderError, K8sResourceReceipt};

type ImageUserCache = Arc<Mutex<HashMap<String, (tokio::time::Instant, Option<String>)>>>;

pub struct K8sMicroVmProvider {
    pub(crate) ops: Arc<dyn K8sOps>,
    pub(crate) config: K8sMicroVmConfig,
    pub(crate) jetstream: Option<async_nats::jetstream::Context>,
    pub(crate) lifecycle: Option<Arc<dyn LifecycleEmitter>>,
    pub(crate) runtime_class_override: Option<String>,
    pub(crate) violation_nats_url: Option<String>,
    /// Per owner identity, the apiserver-clock seconds at which its pod was
    /// first and last seen absent in a run of consecutive sweeps.
    /// Shared by the clones a launch makes, so one sweep sees another's.
    pub(crate) owner_absence: Arc<Mutex<HashMap<String, (i64, i64)>>>,
    /// Looks up an image's own `USER` when [`K8sMicroVmConfig::honor_image_user`]
    /// is on. Without one, every image falls back to [`DEFAULT_AGENT_UID`].
    pub(crate) image_user_resolver: Option<Arc<dyn ImageUserResolver>>,
    /// Successful image `USER` lookups, so a burst of launches from one image
    /// asks the registry once. Shared by the clones a launch makes.
    pub(crate) image_user_cache: ImageUserCache,
    pub(crate) receipts: Option<Arc<Mutex<Vec<K8sResourceReceipt>>>>,
}

impl K8sMicroVmProvider {
    pub(crate) fn new(ops: Arc<dyn K8sOps>, config: K8sMicroVmConfig) -> Self {
        Self {
            ops,
            config,
            jetstream: None,
            lifecycle: None,
            runtime_class_override: None,
            violation_nats_url: None,
            owner_absence: Default::default(),
            image_user_resolver: None,
            image_user_cache: Default::default(),
            receipts: None,
        }
    }

    /// Enables egress violation events from the sidecar. The URL is passed to
    /// the sidecar unchanged, so it must be reachable from inside the cluster.
    pub fn with_violation_nats_url(mut self, nats_url: impl Into<String>) -> Self {
        self.violation_nats_url = Some(nats_url.into());
        self
    }

    /// Supplies the lookup of an image's own `USER` that
    /// [`K8sMicroVmConfig::honor_image_user`] needs to tell a non-root image
    /// from a root one.
    pub fn with_image_user_resolver(mut self, resolver: Arc<dyn ImageUserResolver>) -> Self {
        self.image_user_resolver = Some(resolver);
        self
    }

    pub fn with_client(client: Client, config: K8sMicroVmConfig) -> Self {
        Self::new(Arc::new(RealK8sOps::new(client)), config)
    }

    pub async fn with_default_client(config: K8sMicroVmConfig) -> Result<Self, K8sProviderError> {
        let client = Client::try_default().await.map_err(K8sProviderError::BuildClient)?;
        Ok(Self::with_client(client, config))
    }

    pub fn with_nats(mut self, jetstream: async_nats::jetstream::Context) -> Self {
        self.jetstream = Some(jetstream);
        self
    }

    pub fn with_lifecycle_emitter(&self, lifecycle: Option<Arc<dyn LifecycleEmitter>>) -> Self {
        Self {
            ops: Arc::clone(&self.ops),
            config: self.config.clone(),
            jetstream: self.jetstream.clone(),
            lifecycle,
            runtime_class_override: self.runtime_class_override.clone(),
            violation_nats_url: self.violation_nats_url.clone(),
            owner_absence: Arc::clone(&self.owner_absence),
            image_user_resolver: self.image_user_resolver.clone(),
            image_user_cache: Arc::clone(&self.image_user_cache),
            receipts: self.receipts.clone(),
        }
    }

    /// Clone this provider with a per-launch runtime-class override. Used by
    /// the worker adapter to translate `IsolationTier` into the cluster's
    /// configured RuntimeClass name (e.g. `kata-fc`, `gvisor`).
    pub fn with_runtime_class_override(&self, rc: impl Into<String>) -> Self {
        Self {
            ops: Arc::clone(&self.ops),
            config: self.config.clone(),
            jetstream: self.jetstream.clone(),
            lifecycle: self.lifecycle.clone(),
            runtime_class_override: Some(rc.into()),
            violation_nats_url: self.violation_nats_url.clone(),
            owner_absence: Arc::clone(&self.owner_absence),
            image_user_resolver: self.image_user_resolver.clone(),
            image_user_cache: Arc::clone(&self.image_user_cache),
            receipts: self.receipts.clone(),
        }
    }

    /// Look up the runtime class for an optional tier hint without cloning.
    pub fn runtime_class_for(&self, tier: Option<IsolationTier>) -> &str {
        self.config.runtime_class_for(tier)
    }
}

impl K8sMicroVmProvider {
    /// A still-fresh successful lookup for `image`, if one was made.
    fn cached_image_user(&self, image: &str) -> Option<Option<String>> {
        let cache = self
            .image_user_cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let (at, user) = cache.get(image)?;
        (at.elapsed() < IMAGE_USER_CACHE_TTL).then(|| user.clone())
    }

    /// The uid and gid forced on an agent whose lease names no user. With
    /// `honor_image_user` on, an image whose `USER` is a non-root numeric id
    /// keeps it (`None`, so the container's own user applies); a root, unset,
    /// named or unresolvable `USER` falls back to [`DEFAULT_AGENT_UID`] with
    /// a warning, so the container still satisfies `runAsNonRoot`.
    pub(crate) async fn agent_ids_for_image(&self, image: &str) -> (Option<i64>, Option<i64>) {
        let fallback = (Some(DEFAULT_AGENT_UID), Some(DEFAULT_AGENT_UID));
        if !self.config.honor_image_user {
            return fallback;
        }
        let Some(resolver) = self.image_user_resolver.as_ref() else {
            tracing::warn!(
                image,
                uid = DEFAULT_AGENT_UID,
                "honoring the image USER is on but no image lookup is configured, running as the default uid"
            );
            return fallback;
        };
        let lookup = match self.cached_image_user(image) {
            Some(user) => Ok(Ok(user)),
            None => tokio::time::timeout(IMAGE_USER_LOOKUP_TIMEOUT, resolver.image_user(image)).await,
        };
        let lookup = match lookup {
            Ok(result) => result,
            Err(_) => {
                tracing::warn!(
                    image,
                    timeout = ?IMAGE_USER_LOOKUP_TIMEOUT,
                    uid = DEFAULT_AGENT_UID,
                    "reading the image USER timed out, running as the default uid"
                );
                return fallback;
            }
        };
        if let Ok(user) = &lookup {
            self.image_user_cache
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .insert(image.to_string(), (tokio::time::Instant::now(), user.clone()));
        }
        match lookup {
            Ok(user) if image_user_is_non_root_uid(user.as_deref()) => (None, None),
            Ok(user) => {
                tracing::warn!(
                    image,
                    image_user = ?user,
                    uid = DEFAULT_AGENT_UID,
                    "image USER is root, unset or not a numeric id, running as the default uid"
                );
                fallback
            }
            Err(error) => {
                tracing::warn!(
                    image,
                    error = %error,
                    uid = DEFAULT_AGENT_UID,
                    "could not read the image USER, running as the default uid"
                );
                fallback
            }
        }
    }

    /// The Kubernetes namespace this provider works in.
    pub fn namespace(&self) -> &str {
        &self.config.namespace
    }

    pub(crate) fn ensure_resource_naming(&self, labels: &BTreeMap<String, String>) -> Result<(), K8sProviderError> {
        let other = self.config.resource_naming.other();
        if labels.contains_key(other.job_label())
            || labels.get("app.kubernetes.io/managed-by").map(String::as_str) == Some(other.managed_by())
        {
            return Err(K8sProviderError::ConflictingResourceNaming { other });
        }
        Ok(())
    }
}
