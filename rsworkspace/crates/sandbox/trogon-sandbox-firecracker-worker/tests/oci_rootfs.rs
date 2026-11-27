#[cfg(target_os = "linux")]
use std::io::Seek;
use std::io::Write;
#[cfg(target_os = "linux")]
use std::os::unix::fs::{PermissionsExt, symlink};
#[cfg(target_os = "linux")]
use std::path::Path;
use std::sync::Arc;

use flate2::{Compression, write::GzEncoder};
use httpmock::{Method::GET, MockServer};
use sha2::{Digest as _, Sha256};
use trogon_sandbox_contract::OciImageRef;
#[cfg(target_os = "linux")]
use trogon_sandbox_firecracker_worker::rootfs::{ImageCache, OciRootfsConfig};
use trogon_sandbox_oci::{ContainerPullLimits, OciClient, OciClientConfig};

fn digest(data: &[u8]) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(data)))
}

struct Registry {
    _server: MockServer,
    client: Arc<OciClient>,
    image: OciImageRef,
}

impl Registry {
    fn new(raw_layers: Vec<Vec<u8>>, declared_layer_size: Option<i64>) -> Result<Self, Box<dyn std::error::Error>> {
        let server = MockServer::start();
        let registry = server.address().to_string();
        let config = serde_json::to_vec(&serde_json::json!({
            "architecture":"amd64", "os":"linux",
            "config":{"Env":["IMAGE_VALUE=from-image"],"WorkingDir":"/image-workdir"},
            "rootfs":{"type":"layers","diff_ids":raw_layers.iter().map(|layer| digest(layer)).collect::<Vec<_>>()},
        }))?;
        let config_digest = digest(&config);
        let mut blobs = vec![(config_digest.clone(), config.clone())];
        let mut layers = Vec::new();
        for raw in raw_layers {
            let mut gzip = GzEncoder::new(Vec::new(), Compression::fast());
            gzip.write_all(&raw)?;
            let compressed = gzip.finish()?;
            let layer_digest = digest(&compressed);
            layers.push(serde_json::json!({"mediaType":"application/vnd.docker.image.rootfs.diff.tar.gzip","digest":layer_digest,"size":declared_layer_size.unwrap_or(compressed.len() as i64)}));
            blobs.push((layer_digest, compressed));
        }
        let manifest = serde_json::to_vec(&serde_json::json!({
            "schemaVersion":2, "mediaType":"application/vnd.docker.distribution.manifest.v2+json",
            "config":{"mediaType":"application/vnd.docker.container.image.v1+json","digest":config_digest,"size":config.len()},
            "layers":layers,
        }))?;
        let manifest_digest = digest(&manifest);
        for reference in ["latest".to_string(), manifest_digest.clone()] {
            server.mock(|when, then| {
                when.method(GET)
                    .path(format!("/v2/fixture/image/manifests/{reference}"));
                then.status(200)
                    .header("content-type", "application/vnd.docker.distribution.manifest.v2+json")
                    .header("docker-content-digest", &manifest_digest)
                    .body(manifest.clone());
            });
        }
        for (digest, data) in blobs {
            server.mock(|when, then| {
                when.method(GET).path(format!("/v2/fixture/image/blobs/{digest}"));
                then.status(200).body(data);
            });
        }
        let client = Arc::new(OciClient::new(OciClientConfig {
            insecure_registries: vec![trogon_sandbox_oci::RegistryHost::new(registry.clone())?],
            ..Default::default()
        }));
        Ok(Self {
            _server: server,
            client,
            image: OciImageRef::parse_unqualified(&format!("{registry}/fixture/image:latest")),
        })
    }
}

fn tar_layer(files: &[(&str, &[u8])], links: &[(&str, &str, bool)]) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut builder = tar::Builder::new(Vec::new());
    for (path, data) in files {
        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(0o640);
        header.set_uid(1234);
        header.set_gid(2345);
        header.set_cksum();
        builder.append_data(&mut header, path, *data)?;
    }
    for (path, target, symbolic) in links {
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(if *symbolic {
            tar::EntryType::Symlink
        } else {
            tar::EntryType::Link
        });
        header.set_mode(0o640);
        header.set_uid(1234);
        header.set_gid(2345);
        header.set_size(0);
        header.set_link_name(target)?;
        header.set_cksum();
        builder.append_data(&mut header, path, std::io::empty())?;
    }
    Ok(builder.into_inner()?)
}

#[tokio::test]
async fn export_pins_manifest_and_normalizes_docker_layers_without_changing_blob_content()
-> Result<(), Box<dyn std::error::Error>> {
    let registry = Registry::new(vec![tar_layer(&[("payload", b"content")], &[])?], None)?;
    let image = registry.client.resolve_container_image(&registry.image).await?;
    assert_eq!(image.reference().digest, Some(image.digest().clone()));
    assert!(image.reference().tag.is_none());
    assert_eq!(image.env()["IMAGE_VALUE"], "from-image");
    let destination = tempfile::tempdir()?;
    registry
        .client
        .export_container_layout(&image, destination.path())
        .await?;
    let index: serde_json::Value = serde_json::from_slice(&std::fs::read(destination.path().join("index.json"))?)?;
    let normalized_digest = index["manifests"][0]["digest"].as_str().ok_or("value missing")?;
    let manifest: serde_json::Value = serde_json::from_slice(&std::fs::read(
        destination.path().join("blobs/sha256").join(
            normalized_digest
                .strip_prefix("sha256:")
                .ok_or("digest lacks algorithm")?,
        ),
    )?)?;
    assert_eq!(manifest["mediaType"], "application/vnd.oci.image.manifest.v1+json");
    assert_eq!(
        manifest["layers"][0]["mediaType"],
        "application/vnd.oci.image.layer.v1.tar+gzip"
    );
    let blob_digest = manifest["layers"][0]["digest"].as_str().ok_or("value missing")?;
    let data = std::fs::read(
        destination
            .path()
            .join("blobs/sha256")
            .join(blob_digest.strip_prefix("sha256:").ok_or("digest lacks algorithm")?),
    )?;
    assert_eq!(digest(&data), blob_digest);
    Ok(())
}

#[tokio::test]
async fn download_limits_reject_oversized_descriptors_and_lying_blob_lengths() -> Result<(), Box<dyn std::error::Error>>
{
    let registry = Registry::new(vec![tar_layer(&[("payload", b"content")], &[])?], None)?;
    let error = registry
        .client
        .resolve_container_image_with_limits(
            &registry.image,
            ContainerPullLimits {
                max_blob_bytes: 1,
                max_total_bytes: 1,
            },
        )
        .await
        .err()
        .ok_or("expected an error")?;
    assert!(format!("{error:?}").contains("size limit"));
    let registry = Registry::new(vec![tar_layer(&[("payload", b"content")], &[])?], Some(1))?;
    let image = registry.client.resolve_container_image(&registry.image).await?;
    let destination = tempfile::tempdir()?;
    let error = registry
        .client
        .export_container_layout(&image, destination.path())
        .await
        .err()
        .ok_or("expected an error")?;
    assert!(format!("{error:?}").contains("Content-Length"), "{error}");
    let mut sizes = Vec::new();
    for entry in std::fs::read_dir(destination.path().join("blobs/sha256"))? {
        sizes.push(entry?.metadata()?.len());
    }
    assert!(
        sizes.contains(&0),
        "a mismatched body must be rejected before writing its layer"
    );
    assert!(!destination.path().join("index.json").exists());
    Ok(())
}

#[cfg(target_os = "linux")]
fn static_elf() -> Vec<u8> {
    let mut data = vec![0u8; 121];
    data[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
    data[16..18].copy_from_slice(&2u16.to_le_bytes());
    data[18..20].copy_from_slice(&62u16.to_le_bytes());
    data[20..24].copy_from_slice(&1u32.to_le_bytes());
    data[24..32].copy_from_slice(&0x400078u64.to_le_bytes());
    data[32..40].copy_from_slice(&64u64.to_le_bytes());
    data[52..54].copy_from_slice(&64u16.to_le_bytes());
    data[54..56].copy_from_slice(&56u16.to_le_bytes());
    data[56..58].copy_from_slice(&1u16.to_le_bytes());
    data[64..68].copy_from_slice(&1u32.to_le_bytes());
    data[68..72].copy_from_slice(&5u32.to_le_bytes());
    data[80..88].copy_from_slice(&0x400000u64.to_le_bytes());
    data[96..104].copy_from_slice(&121u64.to_le_bytes());
    data[104..112].copy_from_slice(&121u64.to_le_bytes());
    data
}

#[cfg(target_os = "linux")]
fn config(root: &Path) -> Result<OciRootfsConfig, Box<dyn std::error::Error>> {
    let agent = root.join("agent");
    let init = root.join("init");
    std::fs::write(&agent, static_elf())?;
    std::fs::write(&init, static_elf())?;
    Ok(OciRootfsConfig {
        cache_dir: root.join("cache"),
        guest_agent: agent,
        guest_init: init,
        umoci: "umoci".into(),
        mkfs_ext4: "/usr/sbin/mkfs.ext4".into(),
        disk_size_bytes: 64 * 1024 * 1024,
        max_cache_bytes: 256 * 1024 * 1024,
        max_download_bytes: 16 * 1024 * 1024,
    })
}

#[cfg(target_os = "linux")]
async fn debugfs(disk: &Path, command: &str) -> Result<(String, String), Box<dyn std::error::Error>> {
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        tokio::process::Command::new("/usr/sbin/debugfs")
            .args(["-R", command])
            .arg(disk)
            .kill_on_drop(true)
            .output(),
    )
    .await??;
    assert!(output.status.success());
    Ok((String::from_utf8(output.stdout)?, String::from_utf8(output.stderr)?))
}

#[cfg(target_os = "linux")]
#[tokio::test]
#[ignore = "requires root, umoci, e2fsprogs and mount/PID namespace privileges"]
async fn real_umoci_ext4_preserves_whiteouts_hardlinks_modes_and_private_copy() -> Result<(), Box<dyn std::error::Error>>
{
    let registry = Registry::new(
        vec![
            tar_layer(
                &[
                    ("deleted.txt", b"remove"),
                    ("keep.txt", b"keep"),
                    ("opaque/old.txt", b"old"),
                    ("target", b"payload"),
                ],
                &[],
            )?,
            tar_layer(
                &[
                    (".wh.deleted.txt", b""),
                    ("opaque/.wh..wh..opq", b""),
                    ("opaque/new.txt", b"new"),
                ],
                &[("hard.txt", "target", false)],
            )?,
        ],
        None,
    )?;
    let root = tempfile::tempdir()?;
    let settings = config(root.path())?;
    let cache = ImageCache::new(settings, Arc::clone(&registry.client));
    let base = cache.resolve(&registry.image).await?;
    assert_eq!(debugfs(&base.path, "cat /keep.txt").await?.0, "keep");
    assert_eq!(debugfs(&base.path, "cat /opaque/new.txt").await?.0, "new");
    assert!(
        debugfs(&base.path, "stat /deleted.txt")
            .await?
            .1
            .contains("File not found")
    );
    assert!(
        debugfs(&base.path, "stat /opaque/old.txt")
            .await?
            .1
            .contains("File not found")
    );
    let target = debugfs(&base.path, "stat /target").await?.0;
    let hard = debugfs(&base.path, "stat /hard.txt").await?.0;
    assert_eq!(target.split_whitespace().nth(1), hard.split_whitespace().nth(1));
    assert!(target.contains("User:  1234") || target.contains("User: 1234"));
    assert!(target.contains("0640"));
    assert_eq!(std::fs::metadata(&base.path)?.permissions().mode() & 0o222, 0);
    let private = root.path().join("lease.ext4");
    trogon_sandbox_firecracker_worker::disk::copy_private(&base.path, &private).await?;
    let before = base.rootfs_digest.clone();
    let output = tokio::process::Command::new("/usr/sbin/debugfs")
        .args(["-w", "-R", "rm /keep.txt"])
        .arg(&private)
        .output()
        .await?;
    assert!(output.status.success());
    assert_eq!(debugfs(&base.path, "cat /keep.txt").await?.0, "keep");
    assert_eq!(cache.resolve(&registry.image).await?.rootfs_digest, before);
    Ok(())
}

#[cfg(target_os = "linux")]
#[tokio::test]
#[ignore = "requires root, umoci, e2fsprogs and mount/PID namespace privileges"]
async fn cache_rejects_symlink_injection_mutation_and_a_second_owner() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let sentinel = root.path().join("outside");
    std::fs::create_dir(&sentinel)?;
    std::fs::write(sentinel.join("keep"), b"protected")?;
    let hostile = Registry::new(
        vec![tar_layer(
            &[],
            &[(".sandbox", sentinel.to_str().ok_or("non-utf8 path")?, true)],
        )?],
        None,
    )?;
    let settings = config(root.path())?;
    let cache = ImageCache::new(settings.clone(), Arc::clone(&hostile.client));
    assert!(cache.resolve(&hostile.image).await.is_err());
    assert_eq!(std::fs::read_dir(&sentinel)?.count(), 1);
    drop(cache);
    let registry = Registry::new(vec![tar_layer(&[("payload", b"content")], &[])?], None)?;
    let cache = ImageCache::new(settings.clone(), Arc::clone(&registry.client));
    let base = cache.resolve(&registry.image).await?;
    let second = ImageCache::new(settings, Arc::clone(&registry.client));
    assert!(
        second
            .resolve(&registry.image)
            .await
            .err()
            .ok_or("expected an error")?
            .to_string()
            .contains("another worker")
    );
    std::fs::set_permissions(&base.path, std::fs::Permissions::from_mode(0o600))?;
    {
        let mut file = std::fs::OpenOptions::new().write(true).open(&base.path)?;
        file.seek(std::io::SeekFrom::Start(8192))?;
        file.write_all(b"mutation")?;
    }
    std::fs::set_permissions(&base.path, std::fs::Permissions::from_mode(0o444))?;
    assert!(
        cache
            .resolve(&registry.image)
            .await
            .err()
            .ok_or("expected an error")?
            .to_string()
            .contains("integrity")
    );
    std::fs::remove_file(&base.path)?;
    symlink(sentinel.join("keep"), &base.path)?;
    assert!(
        cache
            .resolve(&registry.image)
            .await
            .err()
            .ok_or("expected an error")?
            .to_string()
            .contains("regular file")
    );
    assert_eq!(std::fs::read(sentinel.join("keep"))?, b"protected");
    Ok(())
}

#[cfg(target_os = "linux")]
#[tokio::test]
#[ignore = "requires root, umoci, e2fsprogs and mount/PID namespace privileges"]
async fn bounded_scratch_rejects_expansion_and_cache_budget_without_leaving_artifacts()
-> Result<(), Box<dyn std::error::Error>> {
    let large = vec![0u8; 80 * 1024 * 1024];
    let registry = Registry::new(vec![tar_layer(&[("too-large", &large)], &[])?], None)?;
    let root = tempfile::tempdir()?;
    let settings = config(root.path())?;
    let cache = ImageCache::new(settings.clone(), Arc::clone(&registry.client));
    let error = cache.resolve(&registry.image).await.err().ok_or("expected an error")?;
    assert!(format!("{error:?}").contains("bounded OCI extraction"), "{error:#}");
    assert!(
        std::fs::read_dir(&settings.cache_dir)?
            .all(|entry| entry.is_ok_and(|entry| entry.file_name() == ".worker.lock"))
    );
    drop(cache);
    let mut settings = settings;
    settings.max_cache_bytes = 1;
    let cache = ImageCache::new(settings.clone(), Arc::clone(&registry.client));
    assert!(
        cache
            .resolve(&registry.image)
            .await
            .err()
            .ok_or("expected an error")?
            .to_string()
            .contains("byte budget")
    );
    assert!(
        std::fs::read_dir(&settings.cache_dir)?
            .all(|entry| matches!(entry, Ok(entry) if entry.file_name() == ".worker.lock"))
    );
    Ok(())
}
