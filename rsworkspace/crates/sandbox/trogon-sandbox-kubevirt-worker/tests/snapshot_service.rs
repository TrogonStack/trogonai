mod fake_cluster;
mod support;

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;
use object_store::memory::InMemory;
use object_store::path::Path as ObjectPath;
use object_store::{ObjectStore, ObjectStoreExt};
use serde_json::{Value, json};
use trogon_sandbox_contract::{ArtifactType, LeaseId, OciArtifactRef, S3BucketName};
use trogon_sandbox_cosign::CosignVerifier;
use trogon_sandbox_kubevirt_worker::{
    ExportError, KubeVirtConfig, KubeVirtOps, RestoreError, RestoreStores, SnapshotBackend, SnapshotRegistry,
    SnapshotService, WarmDiskSource,
};
use trogon_sandbox_oci::{Layer, OciClient, OciClientConfig, OciError, RegistryHost};
use trogon_sandbox_volumes::{VolumeBackend, VolumeClientConfig, VolumeEndpoint, VolumeRegion};

use fake_cluster::FakeCluster;
use support::{Boxed, TestResult, config, digest, reference, text};

const LAYER: &str = "the qcow2 bytes";

/// A registry that holds the layers it was given and keeps what is pushed to it.
#[derive(Default)]
struct FakeRegistry {
    layers: Mutex<HashMap<String, Vec<Layer>>>,
    pushed: Mutex<Vec<Layer>>,
    pull_failure: bool,
    push_failures: AtomicUsize,
}

impl FakeRegistry {
    fn holding(reference: &OciArtifactRef, layers: Vec<Layer>) -> Self {
        let registry = Self::default();
        registry
            .layers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(reference.digest.as_str().to_owned(), layers);
        registry
    }

    fn failing_pushes(self, times: usize) -> Self {
        self.push_failures.store(times, Ordering::SeqCst);
        self
    }

    fn pushed(&self) -> Vec<Layer> {
        self.pushed.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

fn layer(data: &str) -> Layer {
    Layer {
        data: data.as_bytes().to_vec(),
        media_type: "application/octet-stream".to_owned(),
        annotations: Default::default(),
    }
}

#[async_trait]
impl SnapshotRegistry for FakeRegistry {
    async fn pull(&self, reference: &OciArtifactRef) -> Result<Vec<Layer>, OciError> {
        if self.pull_failure {
            return Err(OciError::RegistryRejected("the registry is down".to_owned()));
        }
        Ok(self
            .layers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(reference.digest.as_str())
            .cloned()
            .unwrap_or_default())
    }

    async fn push(&self, target: &OciArtifactRef, layer: Layer) -> Result<OciArtifactRef, OciError> {
        let refused = self
            .push_failures
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| left.checked_sub(1))
            .is_ok();
        if refused {
            return Err(OciError::RegistryRejected("the push was refused".to_owned()));
        }
        self.pushed.lock().unwrap_or_else(PoisonError::into_inner).push(layer);
        Ok(OciArtifactRef {
            digest: digest('c').map_err(|error| OciError::RegistryRejected(error.to_string()))?,
            ..target.clone()
        })
    }
}

/// Stores that keep everything in memory and remember which bucket they were opened for.
#[derive(Default)]
struct MemoryStores {
    store: Arc<InMemory>,
    opened: Mutex<Vec<(String, String, Option<String>)>>,
}

impl RestoreStores for MemoryStores {
    fn open(
        &self,
        bucket: &S3BucketName,
        region: &VolumeRegion,
        endpoint: Option<&VolumeEndpoint>,
    ) -> Result<Arc<dyn ObjectStore>, RestoreError> {
        self.opened.lock().unwrap_or_else(PoisonError::into_inner).push((
            bucket.as_str().to_owned(),
            region.as_str().to_owned(),
            endpoint.map(|endpoint| endpoint.as_str().to_owned()),
        ));
        Ok(self.store.clone())
    }
}

fn s3_volume() -> Boxed<VolumeClientConfig> {
    Ok(VolumeClientConfig {
        backend: VolumeBackend::S3 {
            bucket: S3BucketName::new("restore-bucket")?,
            region: VolumeRegion::new("eu-west-1")?,
            endpoint: Some(VolumeEndpoint::new("http://minio.local:9000")?),
        },
        kms_key_id: None,
    })
}

fn ops_with(cluster: &FakeCluster, change: impl FnOnce(&mut KubeVirtConfig)) -> Boxed<KubeVirtOps> {
    let mut config = config(&cluster.namespace())?;
    change(&mut config);
    Ok(KubeVirtOps::new(cluster.client(), config))
}

fn service(registry: &Arc<FakeRegistry>, stores: &Arc<MemoryStores>) -> SnapshotService {
    SnapshotService::new(registry.clone(), stores.clone())
}

#[tokio::test]
async fn a_restore_stages_the_disk_in_s3_and_describes_a_datavolume() -> TestResult {
    let cluster = FakeCluster::new();
    let ops = ops_with(&cluster, |_| {})?;
    let source = reference(&digest('a')?);
    let registry = Arc::new(FakeRegistry::holding(&source, vec![layer(LAYER), layer("ignored")]));
    let stores = Arc::new(MemoryStores::default());
    let labels = BTreeMap::from([("team".to_owned(), "blue".to_owned())]);

    let data_volume = service(&registry, &stores)
        .data_volume(&ops, &s3_volume()?, &source, "dv-1", labels)
        .await?;

    let value = serde_json::to_value(&data_volume)?;
    let key = format!("snapshots/{}/dv-1.qcow2", "a".repeat(64));
    assert_eq!(
        text(&value, "/spec/source/s3/url"),
        Some(format!("s3://restore-bucket/{key}").as_str())
    );
    assert_eq!(value.pointer("/spec/source/s3/secretRef"), None);
    assert_eq!(text(&value, "/metadata/labels/team"), Some("blue"));
    let staged = stores.store.get(&ObjectPath::from(key)).await?.bytes().await?;
    assert_eq!(staged.as_ref(), LAYER.as_bytes());
    assert_eq!(
        stores.opened.lock().unwrap_or_else(PoisonError::into_inner).clone(),
        [(
            "restore-bucket".to_owned(),
            "eu-west-1".to_owned(),
            Some("http://minio.local:9000".to_owned())
        )]
    );
    Ok(())
}

#[tokio::test]
async fn a_configured_secret_is_named_by_the_import_source() -> TestResult {
    let cluster = FakeCluster::new();
    let ops = ops_with(&cluster, |config| {
        config.s3_secret_name = Some("s3-credentials".to_owned());
        config.restore_bucket_prefix = "/staging/".to_owned();
    })?;
    let source = reference(&digest('b')?);
    let registry = Arc::new(FakeRegistry::holding(&source, vec![layer(LAYER)]));
    let stores = Arc::new(MemoryStores::default());

    let data_volume = service(&registry, &stores)
        .data_volume(&ops, &s3_volume()?, &source, "dv-2", BTreeMap::new())
        .await?;

    let value = serde_json::to_value(&data_volume)?;
    assert_eq!(text(&value, "/spec/source/s3/secretRef"), Some("s3-credentials"));
    let key = format!("staging/{}/dv-2.qcow2", "b".repeat(64));
    assert!(stores.store.get(&ObjectPath::from(key)).await.is_ok());
    Ok(())
}

#[tokio::test]
async fn a_restore_fails_when_it_cannot_be_staged() -> TestResult {
    let cluster = FakeCluster::new();
    let ops = ops_with(&cluster, |_| {})?;
    let source = reference(&digest('a')?);
    let stores = Arc::new(MemoryStores::default());
    let empty = Arc::new(FakeRegistry::holding(&source, Vec::new()));
    let outcome = service(&empty, &stores)
        .data_volume(&ops, &s3_volume()?, &source, "dv-1", BTreeMap::new())
        .await;
    assert!(outcome.is_err());

    let down = Arc::new(FakeRegistry {
        pull_failure: true,
        ..FakeRegistry::default()
    });
    let outcome = service(&down, &stores)
        .data_volume(&ops, &s3_volume()?, &source, "dv-1", BTreeMap::new())
        .await;
    assert!(outcome.is_err());

    let local = VolumeClientConfig {
        backend: VolumeBackend::LocalFs {
            root: std::env::temp_dir(),
        },
        kms_key_id: None,
    };
    let outcome = service(&empty, &stores)
        .data_volume(&ops, &local, &source, "dv-1", BTreeMap::new())
        .await;
    assert!(outcome.is_err());
    assert!(stores.opened.lock().unwrap_or_else(PoisonError::into_inner).is_empty());
    Ok(())
}

#[tokio::test]
async fn a_signature_that_cannot_be_verified_stops_the_restore() -> TestResult {
    let cluster = FakeCluster::new();
    let ops = ops_with(&cluster, |config| {
        config.cosign_verifier = Some(CosignVerifier::from_pem(b"not a public key".to_vec()));
    })?;
    let source = reference(&digest('a')?);
    let registry = Arc::new(FakeRegistry::holding(&source, vec![layer(LAYER)]));
    let stores = Arc::new(MemoryStores::default());
    let outcome = service(&registry, &stores)
        .data_volume(&ops, &s3_volume()?, &source, "dv-1", BTreeMap::new())
        .await;
    assert!(outcome.is_err());
    assert!(stores.opened.lock().unwrap_or_else(PoisonError::into_inner).is_empty());
    Ok(())
}

fn vm(name: &str, running: bool) -> Value {
    json!({
        "apiVersion": "kubevirt.io/v1",
        "kind": "VirtualMachine",
        "metadata": {"name": name},
        "spec": {"running": running, "template": {"metadata": {}, "spec": {}}},
    })
}

struct Export {
    cluster: FakeCluster,
    ops: KubeVirtOps,
    lease: LeaseId,
    vm_name: String,
    target: OciArtifactRef,
}

fn export_fixture(running: bool) -> Boxed<Export> {
    let cluster = FakeCluster::new();
    let ops = ops_with(&cluster, |_| {})?;
    let vm_name = ops.vm_name("lease-1");
    cluster.insert("virtualmachines", vm(&vm_name, running));
    if running {
        cluster.run_vmi(&vm_name);
    }
    cluster.set_pvc_contents(&format!("{}-rootdisk", ops.snapshot_name("lease-1")), LAYER);
    let mut target = reference(&digest('d')?);
    target.artifact_type = ArtifactType::KubevirtDiskV1;
    Ok(Export {
        cluster,
        ops,
        lease: LeaseId::new("lease-1"),
        vm_name,
        target,
    })
}

#[tokio::test(start_paused = true)]
async fn a_running_vm_is_exported_without_stopping_it() -> TestResult {
    let fixture = export_fixture(true)?;
    let registry = Arc::new(FakeRegistry::default());
    let stores = Arc::new(MemoryStores::default());
    let receipt = service(&registry, &stores)
        .export(
            &fixture.ops,
            &fixture.lease,
            &fixture.vm_name,
            BTreeMap::new(),
            &fixture.target,
        )
        .await
        .map_err(|error| error.to_string())?;
    assert_eq!(receipt.bytes, LAYER.len() as u64);
    assert_eq!(receipt.digest, digest('c')?);
    assert_eq!(receipt.artifact_type, ArtifactType::KubevirtDiskV1);
    let pushed = registry.pushed();
    assert_eq!(pushed.len(), 1);
    assert_eq!(pushed[0].data, LAYER.as_bytes());
    assert_eq!(pushed[0].media_type, ArtifactType::KubevirtDiskV1.media_type());
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_failed_online_export_falls_back_to_stopping_the_vm() -> TestResult {
    let fixture = export_fixture(true)?;
    let registry = Arc::new(FakeRegistry::default().failing_pushes(1));
    let stores = Arc::new(MemoryStores::default());
    let receipt = service(&registry, &stores)
        .export(
            &fixture.ops,
            &fixture.lease,
            &fixture.vm_name,
            BTreeMap::new(),
            &fixture.target,
        )
        .await
        .map_err(|error| error.to_string())?;
    assert_eq!(receipt.bytes, LAYER.len() as u64);
    assert_eq!(registry.pushed().len(), 1);
    let stored = fixture
        .cluster
        .get("virtualmachines", &fixture.vm_name)
        .ok_or("the VM is gone")?;
    assert_eq!(stored.pointer("/spec/running"), Some(&json!(true)));
    assert!(fixture.ops.vmi_is_running(&fixture.vm_name).await?);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_stopped_vm_is_exported_offline() -> TestResult {
    let fixture = export_fixture(false)?;
    let registry = Arc::new(FakeRegistry::default());
    let stores = Arc::new(MemoryStores::default());
    service(&registry, &stores)
        .export(
            &fixture.ops,
            &fixture.lease,
            &fixture.vm_name,
            BTreeMap::new(),
            &fixture.target,
        )
        .await
        .map_err(|error| error.to_string())?;
    assert_eq!(registry.pushed().len(), 1);
    assert!(!fixture.ops.vmi_is_running(&fixture.vm_name).await?);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn an_export_that_fails_both_ways_reports_the_registry_error() -> TestResult {
    let fixture = export_fixture(true)?;
    let registry = Arc::new(FakeRegistry::default().failing_pushes(2));
    let stores = Arc::new(MemoryStores::default());
    let outcome = service(&registry, &stores)
        .export(
            &fixture.ops,
            &fixture.lease,
            &fixture.vm_name,
            BTreeMap::new(),
            &fixture.target,
        )
        .await;
    assert!(matches!(
        outcome
            .err()
            .and_then(|error| error.downcast::<ExportError>().ok())
            .as_deref(),
        Some(ExportError::Registry(_))
    ));
    assert!(fixture.ops.vmi_is_running(&fixture.vm_name).await?);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_checkpoint_needs_a_running_vm_and_never_stops_it() -> TestResult {
    let stopped = export_fixture(false)?;
    let registry = Arc::new(FakeRegistry::default());
    let stores = Arc::new(MemoryStores::default());
    let outcome = service(&registry, &stores)
        .checkpoint(
            &stopped.ops,
            &stopped.lease,
            &stopped.vm_name,
            BTreeMap::new(),
            &stopped.target,
        )
        .await;
    assert!(matches!(
        outcome
            .err()
            .and_then(|error| error.downcast::<ExportError>().ok())
            .as_deref(),
        Some(ExportError::NotRunning { .. })
    ));

    let running = export_fixture(true)?;
    let receipt = service(&registry, &stores)
        .checkpoint(
            &running.ops,
            &running.lease,
            &running.vm_name,
            BTreeMap::new(),
            &running.target,
        )
        .await
        .map_err(|error| error.to_string())?;
    assert_eq!(receipt.bytes, LAYER.len() as u64);
    assert!(running.ops.vmi_is_running(&running.vm_name).await?);
    Ok(())
}

fn unreachable_client() -> Boxed<(OciClient, OciArtifactRef)> {
    let host = RegistryHost::new("127.0.0.1:1")?;
    let client = OciClient::new(OciClientConfig {
        insecure_registries: vec![host],
        ..OciClientConfig::default()
    });
    let mut target = reference(&digest('e')?);
    target.registry = "127.0.0.1:1".to_owned();
    Ok((client, target))
}

#[tokio::test]
async fn the_oci_client_reports_an_unreachable_registry() -> TestResult {
    let (client, target) = unreachable_client()?;
    assert!(SnapshotRegistry::pull(&client, &target).await.is_err());
    assert!(SnapshotRegistry::push(&client, &target, layer(LAYER)).await.is_err());
    Ok(())
}
