//! Round trip of the three volume modes against a real S3-compatible store (RustFS in Docker).
//!
//! ```text
//! cargo test -p trogon-sandbox-volumes --test s3_roundtrip -- --ignored --nocapture
//! ```

use std::error::Error;
use std::net::TcpListener;
use std::process::Stdio;
use std::time::Duration;

use bytes::Bytes;
use object_store::aws::AmazonS3Builder;
use object_store::path::Path as ObjectPath;
use object_store::{ObjectStore, ObjectStoreExt, PutPayload};
use tempfile::tempdir;
use tokio::process::Command;
use trogon_sandbox_contract::S3BucketName;
use trogon_sandbox_volumes::{
    VolumeBackend, VolumeClient, VolumeClientConfig, VolumeEndpoint, VolumeMode, VolumePrefix, VolumeRegion,
    build_changed_manifest,
};

type TestResult = Result<(), Box<dyn Error>>;

const S3_IMAGE: &str = "rustfs/rustfs:latest";
const ACCESS_KEY: &str = "rustfsadmin";
const REGION: &str = "us-east-1";
const READY_ATTEMPTS: u32 = 120;

struct S3Store {
    port: u16,
    bucket: String,
    container_id: String,
}

impl S3Store {
    async fn start() -> Result<Self, Box<dyn Error>> {
        let port = TcpListener::bind("127.0.0.1:0")?.local_addr()?.port();
        let bucket = format!("volume-test-{port}");
        let output = Command::new("docker")
            .args(["run", "-d", "--rm", "-p"])
            .arg(format!("{port}:9000"))
            .args(["-e", "RUSTFS_ADDRESS=:9000"])
            .args(["--entrypoint", "sh", S3_IMAGE, "-c"])
            .arg(format!("mkdir -p /data/{bucket} && exec /entrypoint.sh /data"))
            .output()
            .await?;
        if !output.status.success() {
            return Err(format!("docker run failed: {}", String::from_utf8_lossy(&output.stderr)).into());
        }
        let container_id = String::from_utf8(output.stdout)?.trim().to_string();
        let store = Self {
            port,
            bucket,
            container_id,
        };
        store.wait_until_ready().await?;
        Ok(store)
    }

    fn endpoint(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    fn store(&self) -> Result<impl ObjectStore + use<>, Box<dyn Error>> {
        Ok(AmazonS3Builder::new()
            .with_bucket_name(&self.bucket)
            .with_region(REGION)
            .with_endpoint(self.endpoint())
            .with_allow_http(true)
            .with_access_key_id(ACCESS_KEY)
            .with_secret_access_key(ACCESS_KEY)
            .build()?)
    }

    async fn wait_until_ready(&self) -> TestResult {
        let store = self.store()?;
        for _ in 0..READY_ATTEMPTS {
            let probe = PutPayload::from_bytes(Bytes::from_static(b"x"));
            if store.put(&ObjectPath::from(".ready"), probe).await.is_ok() {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        Err(format!("the S3 store did not become ready on port {}", self.port).into())
    }

    fn config(&self) -> Result<VolumeClientConfig, Box<dyn Error>> {
        Ok(VolumeClientConfig {
            backend: VolumeBackend::S3 {
                bucket: S3BucketName::new(&self.bucket)?,
                region: VolumeRegion::new(REGION)?,
                endpoint: Some(VolumeEndpoint::new(self.endpoint())?),
            },
            kms_key_id: None,
        })
    }

    async fn stop(self) {
        let _ = Command::new("docker")
            .args(["stop", &self.container_id])
            .stdout(Stdio::null())
            .output()
            .await;
    }
}

async fn seed(store: &dyn ObjectStore, prefix: &str) -> TestResult {
    let files: [(&str, &[u8]); 3] = [
        ("README.md", b"# demo"),
        ("src/main.rs", b"fn main() {}"),
        ("docs/guide.md", b"guide"),
    ];
    for (relative, body) in files {
        let payload = PutPayload::from_bytes(Bytes::copy_from_slice(body));
        store
            .put(&ObjectPath::from(format!("{prefix}/{relative}")), payload)
            .await?;
    }
    Ok(())
}

async fn every_mode_round_trips(server: &S3Store) -> TestResult {
    let store = server.store()?;
    seed(&store, "workspace/base").await?;
    let base = VolumePrefix::new("workspace/base")?;

    let read_only = VolumeClient::with_mode(server.config()?, VolumeMode::ReadOnly)?;
    let read_dest = tempdir()?;
    let pulled = read_only.pull_to_dir(&base, read_dest.path()).await?;
    assert_eq!(pulled.objects, 3);
    assert_eq!(std::fs::read(read_dest.path().join("src/main.rs"))?, b"fn main() {}");
    assert!(read_only.head_exists(&base).await?);

    let read_write = VolumeClient::with_mode(server.config()?, VolumeMode::ReadWrite)?;
    let rw_dest = tempdir()?;
    read_write.pull_to_dir(&base, rw_dest.path()).await?;
    std::fs::write(rw_dest.path().join("src/main.rs"), b"fn main() { println!(\"hi\"); }")?;
    let mut manifest = build_changed_manifest(rw_dest.path(), None)?;
    manifest.entries.retain(|entry| entry.key.as_str() == "src/main.rs");
    let pushed = read_write
        .push_changed(&VolumePrefix::new("workspace/rw")?, rw_dest.path(), manifest)
        .await?;
    assert_eq!(pushed.objects, 1);
    let uploaded = store
        .get(&ObjectPath::from("workspace/rw/src/main.rs"))
        .await?
        .bytes()
        .await?;
    assert_eq!(uploaded.as_ref(), b"fn main() { println!(\"hi\"); }");

    let copy_on_write = VolumeClient::with_mode(server.config()?, VolumeMode::CopyOnWrite)?;
    let cow_dest = tempdir()?;
    let handle = copy_on_write
        .cow_setup(&base, &VolumePrefix::new("workspace/lease-1")?, cow_dest.path())
        .await?;
    std::fs::write(handle.delta_dir.join("delta.txt"), b"lease delta")?;
    let expected = build_changed_manifest(&handle.delta_dir, None)?.digest()?;
    let digest = copy_on_write.cow_finalize(handle).await?;
    assert_eq!(digest, expected);
    let delta = store
        .get(&ObjectPath::from("workspace/lease-1/delta.txt"))
        .await?
        .bytes()
        .await?;
    assert_eq!(delta.as_ref(), b"lease delta");
    Ok(())
}

#[test]
#[ignore = "requires Docker and the RustFS image; run with --ignored"]
fn s3_backed_volume_round_trips_in_every_mode() -> TestResult {
    // SAFETY: this binary holds one test and nothing else runs yet; the runtime is built afterwards.
    unsafe {
        std::env::set_var("AWS_ACCESS_KEY_ID", ACCESS_KEY);
        std::env::set_var("AWS_SECRET_ACCESS_KEY", ACCESS_KEY);
        std::env::set_var("AWS_REGION", REGION);
    }
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(async {
            let server = S3Store::start().await?;
            let outcome = every_mode_round_trips(&server).await;
            server.stop().await;
            outcome
        })
}
