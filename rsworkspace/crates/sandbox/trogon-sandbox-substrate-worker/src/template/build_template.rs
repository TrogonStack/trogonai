use super::TemplateKey;
use super::constants::{AGENT_BINARY_PATH, AGENT_CONTAINER, WORKSPACE_MOUNT_PATH, WORKSPACE_VOLUME};
use crate::config::{SANDBOX_CLASS_LABEL, SIZE_CLASS_LABEL, SubstrateConfig};
use crate::proto;

/// The immutable workload definition: one container running the agent, one durable dir.
///
/// The tenant's image is the container image; the agent is what actually runs, because the
/// agent is the only exec transport an actor has. The lease's own command is executed by the
/// agent, not by the container entrypoint.
pub(super) fn build_template(config: &SubstrateConfig, atespace: &str, key: &TemplateKey) -> proto::ActorTemplate {
    proto::ActorTemplate {
        metadata: Some(proto::ResourceMetadata {
            atespace: atespace.to_string(),
            name: key.template_name(),
            ..Default::default()
        }),
        worker_selector: Some(proto::Selector {
            match_labels: [
                (SIZE_CLASS_LABEL.to_string(), key.size_class_label.clone()),
                (SANDBOX_CLASS_LABEL.to_string(), key.sandbox_class.as_str().to_string()),
            ]
            .into(),
        }),
        containers: vec![proto::Container {
            name: AGENT_CONTAINER.to_string(),
            image: key.image_reference().to_string(),
            command: vec![AGENT_BINARY_PATH.to_string()],
            args: vec![format!("--port={}", config.agent_port)],
            env: vec![],
            wakeup_probe: Some(proto::WakeupProbe {
                http_get: Some(proto::HttpGetAction {
                    path: trogon_sandbox_actor_agent::READYZ_PATH.to_string(),
                    port: i32::from(config.agent_port),
                }),
                timeout_seconds: 30,
            }),
            volume_mounts: vec![proto::VolumeMount {
                name: WORKSPACE_VOLUME.to_string(),
                mount_path: WORKSPACE_MOUNT_PATH.to_string(),
            }],
            security_context: None,
            resources: None,
        }],
        volumes: vec![proto::Volume {
            name: WORKSPACE_VOLUME.to_string(),
            durable_dir: Some(proto::DurableDirVolumeSource {}),
            external: None,
            ..Default::default()
        }],
        snapshot_config: Some(proto::SnapshotConfig {
            storage_location: key.snapshot_storage_location.clone(),
            preferred_fidelity: proto::SnapshotFidelity::Memory as i32,
            golden_snapshot_config: None,
        }),
        resources: Some(key.resources.as_proto()),
        golden_egress_policy: None,
        status: None,
    }
}
