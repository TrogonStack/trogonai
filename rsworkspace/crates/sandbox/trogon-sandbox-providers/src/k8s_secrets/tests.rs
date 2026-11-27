use trogon_sandbox_contract::SecretRef;

use super::{
    build_clone_secret, build_git_init_container, build_workspace_import_init_container, build_workspace_tar_secret,
    build_workspace_tar_secret_with_naming, resolve_secret_bytes,
};
use crate::K8sResourceNaming;
use crate::K8sSecretError;

#[tokio::test]
async fn a_literal_reference_resolves_to_its_own_bytes() -> Result<(), K8sSecretError> {
    let bytes = resolve_secret_bytes(
        &SecretRef::Literal {
            value: "token-1".into(),
        },
        None,
    )
    .await?;

    assert_eq!(bytes, b"token-1");
    Ok(())
}

#[tokio::test]
async fn a_kv_reference_needs_a_jetstream_context() {
    let reference = SecretRef::NatsKv {
        bucket: "sandbox-secrets".into(),
        key: "clone".into(),
    };

    let result = resolve_secret_bytes(&reference, None).await;

    assert!(matches!(result, Err(K8sSecretError::JetStreamRequired)));
}

#[test]
fn the_clone_secret_carries_the_token_and_the_job_labels() {
    let secret = build_clone_secret("job-clone-creds", "work", "job", b"abc", K8sResourceNaming::Sandbox);

    assert_eq!(secret.metadata.name.as_deref(), Some("job-clone-creds"));
    assert_eq!(secret.metadata.namespace.as_deref(), Some("work"));
    assert_eq!(secret.type_.as_deref(), Some("Opaque"));
    let labels = secret.metadata.labels.unwrap_or_default();
    assert_eq!(
        labels.get("app.kubernetes.io/managed-by").map(String::as_str),
        Some("sandbox-worker")
    );
    assert_eq!(labels.get("sandbox.trogon.live/job").map(String::as_str), Some("job"));
    let data = secret.data.unwrap_or_default();
    assert_eq!(data.get("token").map(|value| value.0.clone()), Some(b"abc".to_vec()));
}

#[test]
fn the_workspace_secret_defaults_to_the_sandbox_naming() {
    let secret = build_workspace_tar_secret("job-ws", "work", "job", b"tar");

    let labels = secret.metadata.labels.unwrap_or_default();
    assert_eq!(
        labels.get("app.kubernetes.io/managed-by").map(String::as_str),
        Some("sandbox-worker")
    );
    let data = secret.data.unwrap_or_default();
    assert_eq!(
        data.get("workspace.tar").map(|value| value.0.clone()),
        Some(b"tar".to_vec())
    );
}

#[test]
fn the_workspace_secret_follows_the_requested_naming() {
    let secret = build_workspace_tar_secret_with_naming("job-ws", "work", "job", b"tar", K8sResourceNaming::Legacy);

    let labels = secret.metadata.labels.unwrap_or_default();
    assert_eq!(
        labels.get("app.kubernetes.io/managed-by").map(String::as_str),
        Some("legacy-runner")
    );
    assert_eq!(labels.get("legacy/job").map(String::as_str), Some("job"));
}

#[test]
fn the_import_init_container_unpacks_the_tar_into_the_workspace() {
    let container = build_workspace_import_init_container("import", "import-volume");

    assert_eq!(container.name, "import");
    let command = container.command.unwrap_or_default();
    assert!(command[2].contains("tar xf /import/workspace.tar -C /workspace"));
    let mounts = container.volume_mounts.unwrap_or_default();
    assert_eq!(mounts[0].name, "import-volume");
    assert_eq!(mounts[0].mount_path, "/import");
    assert_eq!(mounts[1].mount_path, "/workspace");
    let security = container.security_context.unwrap_or_default();
    assert_eq!(security.run_as_non_root, Some(false));
}

#[test]
fn the_git_init_container_clones_and_checks_out_the_resolved_sha() {
    let container = build_git_init_container("https://example.com/o/r.git", "abc123", "job-clone-creds");

    let command = container.command.unwrap_or_default();
    assert!(command[2].contains("git -C /workspace checkout abc123"));
    let env = container.env.unwrap_or_default();
    assert_eq!(env[0].value.as_deref(), Some("https://example.com/o/r.git"));
    let selector = env[1]
        .value_from
        .clone()
        .and_then(|source| source.secret_key_ref)
        .unwrap_or_default();
    assert_eq!(selector.name, "job-clone-creds");
    assert_eq!(selector.key, "token");
    let security = container.security_context.unwrap_or_default();
    assert_eq!(
        security.capabilities.and_then(|caps| caps.drop),
        Some(vec!["ALL".to_string()])
    );
}
