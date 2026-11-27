use std::io::{Read, Write};

use std::collections::BTreeMap;

use anyhow::Context;
use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use trogon_sandbox_contract::ArtifactType;
use trogon_sandbox_oci::{Layer, OciClient, build_push_target};

use crate::checkpoint::{checkpoint_layer, is_checkpoint_layer};

mod constants;

pub use constants::ROOTFS_LAYER_MEDIA_TYPE;

pub fn gzip_tar(raw_tar: &[u8]) -> anyhow::Result<Vec<u8>> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(raw_tar).context("gzip encode rootfs tar failed")?;
    encoder.finish().context("gzip finish rootfs tar failed")
}

pub fn gunzip_tar(gz: &[u8]) -> anyhow::Result<Vec<u8>> {
    let mut decoder = GzDecoder::new(gz);
    let mut out = Vec::new();
    decoder.read_to_end(&mut out).context("gzip decode rootfs tar failed")?;
    Ok(out)
}

pub fn rootfs_layer(gz_tar: Vec<u8>) -> Layer {
    Layer {
        data: gz_tar,
        media_type: ROOTFS_LAYER_MEDIA_TYPE.to_string(),
        annotations: Default::default(),
    }
}

pub async fn push_rootfs_artifact(
    client: &OciClient,
    registry: &str,
    repository: &str,
    gz_tar: Vec<u8>,
) -> anyhow::Result<(trogon_sandbox_contract::OciArtifactRef, u64)> {
    push_artifact_with_layers(
        client,
        registry,
        repository,
        vec![rootfs_layer(gz_tar.clone())],
        gz_tar.len() as u64,
    )
    .await
}

pub async fn push_checkpoint_artifact(
    client: &OciClient,
    registry: &str,
    repository: &str,
    gz_tar: Vec<u8>,
    checkpoint_name: &str,
) -> anyhow::Result<(trogon_sandbox_contract::OciArtifactRef, u64)> {
    push_artifact_with_layers(
        client,
        registry,
        repository,
        vec![checkpoint_layer(gz_tar.clone(), checkpoint_name)],
        gz_tar.len() as u64,
    )
    .await
}

async fn push_artifact_with_layers(
    client: &OciClient,
    registry: &str,
    repository: &str,
    layers: Vec<Layer>,
    bytes: u64,
) -> anyhow::Result<(trogon_sandbox_contract::OciArtifactRef, u64)> {
    let target = build_push_target(
        registry,
        repository,
        ArtifactType::SandboxRootfsV1,
        &layers,
        &Default::default(),
    )
    .map_err(|e| anyhow::anyhow!("build push target: {e}"))?;

    client
        .push_artifact(&target, layers, Default::default())
        .await
        .map_err(|e| anyhow::anyhow!("push artifact: {e}"))?;

    Ok((target, bytes))
}

pub struct PulledSnapshotLayer {
    pub data: Vec<u8>,
    pub annotations: BTreeMap<String, String>,
}

pub async fn pull_snapshot_layer(
    client: &OciClient,
    reference: &trogon_sandbox_contract::OciArtifactRef,
) -> anyhow::Result<PulledSnapshotLayer> {
    let pulled = client
        .pull_artifact(reference)
        .await
        .map_err(|e| anyhow::anyhow!("pull artifact: {e}"))?;
    let layer = pulled
        .layers
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("artifact has no layers"))?;
    Ok(PulledSnapshotLayer {
        data: layer.data,
        annotations: layer.annotations,
    })
}

pub async fn pull_rootfs_layer(
    client: &OciClient,
    reference: &trogon_sandbox_contract::OciArtifactRef,
) -> anyhow::Result<Vec<u8>> {
    Ok(pull_snapshot_layer(client, reference).await?.data)
}

pub fn layer_is_checkpoint(annotations: &BTreeMap<String, String>) -> bool {
    is_checkpoint_layer(annotations)
}
