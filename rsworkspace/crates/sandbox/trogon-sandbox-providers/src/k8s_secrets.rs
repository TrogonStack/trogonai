//! Builders for the Secrets and init containers that bring a workspace into a pod.

mod constants;

use std::collections::BTreeMap;

use k8s_openapi::ByteString;
use k8s_openapi::api::core::v1::{
    Capabilities, Container, EnvVar, EnvVarSource, Secret, SecretKeySelector, SecurityContext, VolumeMount,
};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use trogon_sandbox_contract::SecretRef;

use crate::k8s_ops::constants::{GIT_INIT_IMAGE, WORKSPACE_IMPORT_INIT_IMAGE, WORKSPACE_PATH, WORKSPACE_TAR_KEY};
use crate::{K8sResourceNaming, K8sSecretError};

use constants::CLONE_TOKEN_KEY;

/// Reads the bytes a secret reference points at. A `NatsKv` reference needs `jetstream` to read its bucket.
pub async fn resolve_secret_bytes(
    secret_ref: &SecretRef,
    jetstream: Option<&async_nats::jetstream::Context>,
) -> Result<Vec<u8>, K8sSecretError> {
    match secret_ref {
        SecretRef::Literal { value } => Ok(value.as_bytes().to_vec()),
        SecretRef::NatsKv { bucket, key } => {
            let jetstream = jetstream.ok_or(K8sSecretError::JetStreamRequired)?;
            let store = jetstream
                .get_key_value(bucket)
                .await
                .map_err(|source| K8sSecretError::OpenBucket {
                    bucket: bucket.clone(),
                    source,
                })?;
            let entry = store.get(key).await.map_err(|source| K8sSecretError::Fetch {
                key: key.clone(),
                source,
            })?;
            let entry = entry.ok_or_else(|| K8sSecretError::NotFound {
                key: key.clone(),
                bucket: bucket.clone(),
            })?;
            Ok(entry.to_vec())
        }
    }
}

fn opaque_job_secret(
    secret_name: &str,
    namespace: &str,
    job_name: &str,
    key: &str,
    bytes: &[u8],
    naming: K8sResourceNaming,
) -> Secret {
    let labels = BTreeMap::from([
        (
            "app.kubernetes.io/managed-by".to_string(),
            naming.managed_by().to_string(),
        ),
        (naming.job_label().to_string(), job_name.to_string()),
    ]);

    Secret {
        metadata: ObjectMeta {
            name: Some(secret_name.to_string()),
            namespace: Some(namespace.to_string()),
            labels: Some(labels),
            ..Default::default()
        },
        type_: Some("Opaque".to_string()),
        data: Some(BTreeMap::from([(key.to_string(), ByteString(bytes.to_vec()))])),
        ..Default::default()
    }
}

pub(crate) fn build_clone_secret(
    secret_name: &str,
    namespace: &str,
    job_name: &str,
    token_bytes: &[u8],
    naming: K8sResourceNaming,
) -> Secret {
    opaque_job_secret(secret_name, namespace, job_name, CLONE_TOKEN_KEY, token_bytes, naming)
}

pub fn build_workspace_tar_secret(secret_name: &str, namespace: &str, job_name: &str, tar_bytes: &[u8]) -> Secret {
    build_workspace_tar_secret_with_naming(secret_name, namespace, job_name, tar_bytes, K8sResourceNaming::Sandbox)
}

pub(crate) fn build_workspace_tar_secret_with_naming(
    secret_name: &str,
    namespace: &str,
    job_name: &str,
    tar_bytes: &[u8],
    naming: K8sResourceNaming,
) -> Secret {
    opaque_job_secret(secret_name, namespace, job_name, WORKSPACE_TAR_KEY, tar_bytes, naming)
}

pub(crate) fn build_workspace_import_init_container(name: &str, volume_name: &str) -> Container {
    let script = format!(
        "set -euo pipefail\n\
         tar xf /import/{WORKSPACE_TAR_KEY} -C {WORKSPACE_PATH}\n\
         chmod -R g+rwX {WORKSPACE_PATH}"
    );

    Container {
        name: name.to_string(),
        image: Some(WORKSPACE_IMPORT_INIT_IMAGE.to_string()),
        command: Some(vec!["/bin/sh".to_string(), "-c".to_string(), script]),
        volume_mounts: Some(vec![
            VolumeMount {
                name: volume_name.to_string(),
                mount_path: "/import".to_string(),
                ..Default::default()
            },
            VolumeMount {
                name: "workspace".to_string(),
                mount_path: WORKSPACE_PATH.to_string(),
                ..Default::default()
            },
        ]),
        security_context: Some(SecurityContext {
            run_as_non_root: Some(false),
            read_only_root_filesystem: Some(false),
            ..Default::default()
        }),
        ..Default::default()
    }
}

pub(crate) fn build_git_init_container(clone_url: &str, resolved_sha: &str, secret_name: &str) -> Container {
    let clone_script = format!(
        "set -euo pipefail\n\
         AUTH_URL=$(printf '%s' \"$CLONE_URL\" | sed 's|://|://oauth2:'\\'\"$CLONE_TOKEN\"'\\''@|')\n\
         git clone --depth=1 --filter=blob:none \"$AUTH_URL\" /workspace\n\
         git -C /workspace fetch --depth=1 origin {resolved_sha}\n\
         git -C /workspace checkout {resolved_sha}\n\
         chmod -R g+rwX /workspace"
    );

    Container {
        name: "git-clone".to_string(),
        image: Some(GIT_INIT_IMAGE.to_string()),
        command: Some(vec!["/bin/sh".to_string(), "-c".to_string(), clone_script]),
        env: Some(vec![
            EnvVar {
                name: "CLONE_URL".to_string(),
                value: Some(clone_url.to_string()),
                ..Default::default()
            },
            EnvVar {
                name: "CLONE_TOKEN".to_string(),
                value_from: Some(EnvVarSource {
                    secret_key_ref: Some(SecretKeySelector {
                        name: secret_name.to_string(),
                        key: CLONE_TOKEN_KEY.to_string(),
                        optional: None,
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            },
        ]),
        volume_mounts: Some(vec![VolumeMount {
            name: "workspace".to_string(),
            mount_path: WORKSPACE_PATH.to_string(),
            ..Default::default()
        }]),
        security_context: Some(SecurityContext {
            run_as_non_root: Some(false),
            read_only_root_filesystem: Some(false),
            capabilities: Some(Capabilities {
                drop: Some(vec!["ALL".to_string()]),
                add: None,
            }),
            ..Default::default()
        }),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests;
