use std::fmt::Write as _;

use sha2::{Digest, Sha256};

use super::constants::{TEMPLATE_KEY_HEX_LEN, WORKSPACE_MOUNT_PATH, WORKSPACE_VOLUME};
use crate::config::{SANDBOX_CLASS_LABEL, SandboxClass, SubstrateConfig};
use crate::resources::ActorResources;

/// Everything about a workload that changes what its golden snapshot contains.
///
/// Deliberately small. Per-lease `command`, `args`, and `env` are *not* here: the template
/// runs the agent and the agent applies those at exec time. Baking them in would mean one
/// template per command line (so the cache would never hit) and, for env, would capture a
/// tenant's values inside a golden snapshot shared by every later lease.
///
/// Registry images must carry complete immutable digests before any native template is
/// created. Concrete resource limits and placement constraints also participate in the key.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct TemplateKey {
    pub(super) image_reference: String,
    pub(super) sandbox_class: SandboxClass,
    pub(super) agent_image: String,
    pub(super) agent_port: u16,
    pub(super) volume_shape: String,
    pub(super) size_class_label: String,
    pub(super) resources: ActorResources,
    pub(super) snapshot_storage_location: String,
    pub(super) key: String,
}

impl TemplateKey {
    /// `size_class_label` is the `ate.dev/size` value the golden build, and every actor
    /// instantiated from it, must land on. A template with no size class would let the golden
    /// build schedule onto any pool, including one this tenant's leases never touch; keying on
    /// it means one image gets a separate golden snapshot per size class, matching how a
    /// worker can only ever carry one such label.
    pub fn new(
        image_reference: &str,
        sandbox_class: SandboxClass,
        config: &SubstrateConfig,
        size_class_label: &str,
        resources: ActorResources,
    ) -> Self {
        let agent_image = config.agent_image.as_str();
        let agent_port = config.agent_port;
        let volume_shape = format!("{WORKSPACE_VOLUME}:{WORKSPACE_MOUNT_PATH}:durable_dir");
        let mut hasher = Sha256::new();
        // Length-prefixed so no two field boundaries can be confused for one another.
        for field in [
            "substrate.ateapi.v1",
            image_reference,
            sandbox_class.as_str(),
            agent_image,
            &agent_port.to_string(),
            &volume_shape,
            size_class_label,
            &resources.cpu_millicores().to_string(),
            &resources.memory_mib().to_string(),
            &config.snapshots_location,
            SANDBOX_CLASS_LABEL,
        ] {
            hasher.update((field.len() as u64).to_be_bytes());
            hasher.update(field.as_bytes());
        }
        let digest = hasher.finalize();
        let key = digest
            .iter()
            .fold(String::new(), |mut acc, b| {
                let _ = write!(acc, "{b:02x}");
                acc
            })
            .chars()
            .take(TEMPLATE_KEY_HEX_LEN)
            .collect();

        Self {
            image_reference: image_reference.to_string(),
            sandbox_class,
            agent_image: agent_image.to_string(),
            agent_port,
            volume_shape,
            size_class_label: size_class_label.to_string(),
            resources,
            snapshot_storage_location: config.snapshots_location.clone(),
            key,
        }
    }

    pub fn as_str(&self) -> &str {
        &self.key
    }

    pub fn template_name(&self) -> String {
        format!("tpl-{}", self.key)
    }

    /// Native templates belong to an atespace, so build coordination must share that scope.
    pub fn kv_key(&self, atespace: &str) -> String {
        format!("{}.{}.{}", atespace, self.sandbox_class.as_str(), self.key)
    }

    pub fn image_reference(&self) -> &str {
        &self.image_reference
    }

    pub fn size_class_label(&self) -> &str {
        &self.size_class_label
    }
}
