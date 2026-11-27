//! A real Docker worker provider and a bare node lease request, for the worker lanes that drive the Docker daemon.

use std::sync::Arc;

use anyhow::Context;
use async_nats::Client;
use async_nats::jetstream;
use trogon_sandbox_contract::{
    BootMode, LeaseRequest, NetworkPolicy, OciImageRef, SandboxImage, SandboxSpec, SandboxSpecParts,
};
use trogon_sandbox_providers::LocalDockerProvider;

use crate::adapter::{DockerWorkerProvider, build_oci_client};
use crate::digest_cache::DigestCache;
use crate::support_tests::{lease_request_with, spec_parts};
use crate::warm_pool::WarmPoolConfig;

/// The image every Docker-backed worker lane leases.
pub(super) const NODE_IMAGE: &str = "docker.io/library/node:22-bookworm-slim";

/// A worker provider for `owner` on the local Docker daemon, with the warm pool off.
pub(super) fn docker_provider(client: &Client, nats_url: String, owner: &str) -> anyhow::Result<DockerWorkerProvider> {
    let inner = LocalDockerProvider::with_default_engine(NODE_IMAGE.into())?.with_owner(owner.to_owned());
    docker_provider_over(inner, client, nats_url, owner)
}

/// A worker provider for `owner` over an already configured Docker provider, with the warm pool off.
pub(super) fn docker_provider_over(
    inner: LocalDockerProvider,
    client: &Client,
    nats_url: String,
    owner: &str,
) -> anyhow::Result<DockerWorkerProvider> {
    DockerWorkerProvider::new(
        inner,
        NODE_IMAGE.into(),
        jetstream::new(client.clone()),
        nats_url,
        owner.to_owned(),
        build_oci_client(vec![]),
        "docker.io".into(),
        None,
        Arc::new(DigestCache::new(32)),
        None,
        None,
        WarmPoolConfig {
            enabled: false,
            ..WarmPoolConfig::default()
        },
    )
}

/// The spec fields of a bare-boot node sandbox in `/tmp` with no network, for a lane to change before it leases.
pub(super) fn node_spec_parts() -> anyhow::Result<SandboxSpecParts> {
    let mut parts = spec_parts().map_err(|error| anyhow::anyhow!("{error}"))?;
    parts.image = SandboxImage::Registry {
        reference: OciImageRef::parse_unqualified(NODE_IMAGE),
        pull_secret: None,
    };
    parts.boot_mode = BootMode::Bare;
    parts.working_dir = Some("/tmp".into());
    parts.network_policy = Some(NetworkPolicy::DenyAll);
    Ok(parts)
}

/// A bare-boot node lease in `/tmp` with no network, keyed by `key`.
pub(super) fn node_lease_request(key: &str) -> anyhow::Result<LeaseRequest> {
    node_lease_request_from(key, node_spec_parts()?)
}

/// A lease keyed by `key` for the spec `parts` describe.
pub(super) fn node_lease_request_from(key: &str, parts: SandboxSpecParts) -> anyhow::Result<LeaseRequest> {
    let spec = SandboxSpec::new(parts).context("node lease spec")?;
    lease_request_with(key, spec).map_err(|error| anyhow::anyhow!("{error}"))
}
