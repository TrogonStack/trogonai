#![cfg(target_os = "linux")]

use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use trogon_sandbox_contract::ArtifactType;
use trogon_sandbox_oci::Layer;
use trogon_sandbox_snapshot_builder::security::{
    ScanInput, ScanLimits, ScanPreparation, SecurityError, prepare_scan_input_async, prepare_scan_input_with_options,
};

type Boxed<T> = Result<T, Box<dyn std::error::Error>>;
type TestResult = Boxed<()>;

const ROLE_KEY: &str = "io.trogon.sandbox.firecracker.file";
const ROOTFS_MEDIA_TYPE: &str = "application/vnd.trogon.firecracker.snapshot.rootfs.v1+gzip";

fn preparation(limits: ScanLimits) -> ScanPreparation {
    ScanPreparation {
        limits,
        extractor_program: Some(env!("CARGO_BIN_EXE_trogon-sandbox-snapshot-builder").into()),
    }
}

fn prepare(dir: &Path, artifact: &[Layer], limits: ScanLimits) -> Result<ScanInput, SecurityError> {
    prepare_scan_input_with_options(dir, ArtifactType::FirecrackerSnapshotV1, artifact, &preparation(limits))
}

fn limits_with(change: impl FnOnce(&mut ScanLimits)) -> ScanLimits {
    let mut limits = ScanLimits::default();
    change(&mut limits);
    limits
}

async fn run(command: &mut tokio::process::Command, limit: Duration) -> Boxed<std::process::Output> {
    let output = tokio::time::timeout(limit, command.kill_on_drop(true).output()).await??;
    if !output.status.success() {
        return Err(format!("fixture command failed: {}", String::from_utf8_lossy(&output.stderr)).into());
    }
    Ok(output)
}

async fn image(source: &Path, destination: &Path) -> Boxed<Vec<u8>> {
    std::fs::File::create(destination)?.set_len(32 * 1024 * 1024)?;
    run(
        tokio::process::Command::new("mkfs.ext4")
            .args(["-q", "-F", "-d"])
            .arg(source)
            .arg(destination),
        Duration::from_secs(20),
    )
    .await?;
    let compressed = run(
        tokio::process::Command::new("gzip").arg("-c").arg(destination),
        Duration::from_secs(20),
    )
    .await?;
    Ok(compressed.stdout)
}

fn layers(rootfs: Vec<u8>) -> Vec<Layer> {
    [
        (
            "snapshot.json",
            br#"{"format_version":1,"architecture":"x86_64","memory_mib":128,"vcpus":1}"#.to_vec(),
            "application/json",
        ),
        (
            "snapshot.state",
            b"vmm state must not become a scanned file".to_vec(),
            "application/octet-stream",
        ),
        (
            "snapshot.memory",
            b"guest memory must not become a scanned file".to_vec(),
            "application/octet-stream",
        ),
        ("rootfs.ext4", rootfs, ROOTFS_MEDIA_TYPE),
    ]
    .into_iter()
    .map(|(role, data, media_type)| Layer {
        data,
        media_type: media_type.into(),
        annotations: [(ROLE_KEY.into(), role.into())].into(),
    })
    .collect()
}

fn write_script(path: &Path, body: &str) -> TestResult {
    std::fs::write(path, body)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    Ok(())
}

async fn wait_for<T, F: FnMut() -> Option<T>>(limit: Duration, mut probe: F) -> Boxed<T> {
    tokio::time::timeout(limit, async {
        loop {
            if let Some(value) = probe() {
                return value;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .map_err(Into::into)
}

#[tokio::test]
async fn cancelling_async_preparation_kills_the_test_owned_extractor_process() -> TestResult {
    let scratch = tempfile::tempdir()?;
    let output = scratch.path().join("scan");
    std::fs::create_dir(&output)?;
    let program = scratch.path().join("extractor");
    write_script(
        &program,
        "#!/bin/sh\nprintf '%s' \"$$\" > \"${3}.pid\"\nexec /bin/sleep 30\n",
    )?;
    let pid_path: PathBuf = output.join("rootfs.pid");
    let artifact = layers(vec![1]);
    let task = tokio::spawn(async move {
        let options = ScanPreparation {
            limits: ScanLimits::default(),
            extractor_program: Some(program),
        };
        prepare_scan_input_async(&output, ArtifactType::FirecrackerSnapshotV1, &artifact, &options)
            .await
            .map(|_| ())
    });
    let pid = wait_for(Duration::from_secs(5), || {
        std::fs::read_to_string(&pid_path)
            .ok()
            .and_then(|value| value.parse::<u32>().ok())
    })
    .await?;
    task.abort();
    assert!(task.await.err().is_some_and(|error| error.is_cancelled()));
    wait_for(Duration::from_secs(5), || {
        (!Path::new(&format!("/proc/{pid}")).exists()).then_some(())
    })
    .await?;
    Ok(())
}

#[tokio::test]
async fn scan_extraction_enforces_byte_entry_depth_and_time_budgets() -> TestResult {
    let scratch = tempfile::tempdir()?;
    let source = scratch.path().join("source");
    std::fs::create_dir_all(source.join("dir"))?;
    std::fs::write(source.join("a"), [b'a'; 16])?;
    std::fs::write(source.join("dir/b"), [b'b'; 16])?;
    let artifact = layers(image(&source, &scratch.path().join("rootfs.ext4")).await?);
    let cases = [
        limits_with(|limits| limits.max_compressed_bytes = 1),
        limits_with(|limits| limits.max_image_bytes = 1024 * 1024),
        limits_with(|limits| limits.max_file_bytes = 8),
        limits_with(|limits| {
            limits.max_extracted_bytes = 16;
            limits.max_file_bytes = 16;
        }),
        limits_with(|limits| limits.max_entries = 1),
        limits_with(|limits| limits.max_depth = 1),
        limits_with(|limits| limits.timeout_ms = 0),
    ];
    for (index, limits) in cases.into_iter().enumerate() {
        let output = scratch.path().join(format!("scan-{index}"));
        std::fs::create_dir(&output)?;
        assert!(
            prepare(&output, &artifact, limits).is_err(),
            "scan extraction budget {index} was ignored"
        );
    }
    let sleeper = scratch.path().join("extractor");
    write_script(&sleeper, "#!/bin/sh\nexec /bin/sleep 30\n")?;
    let output = scratch.path().join("deadline");
    std::fs::create_dir(&output)?;
    let started = Instant::now();
    let options = ScanPreparation {
        limits: limits_with(|limits| limits.timeout_ms = 100),
        extractor_program: Some(sleeper),
    };
    let error = match prepare_scan_input_with_options(&output, ArtifactType::FirecrackerSnapshotV1, &artifact, &options)
    {
        Ok(_) => return Err("a stalled extractor must not produce a scan input".into()),
        Err(error) => error,
    };
    assert!(error.to_string().contains("deadline"), "{error}");
    assert!(started.elapsed() < Duration::from_secs(5));
    Ok(())
}

#[tokio::test]
async fn duplicate_snapshot_roles_are_rejected_before_writing_scan_input() -> TestResult {
    let scratch = tempfile::tempdir()?;
    let mut artifact = layers(vec![]);
    artifact[0].annotations.insert(ROLE_KEY.into(), "rootfs.ext4".into());
    assert!(prepare(scratch.path(), &artifact, ScanLimits::default()).is_err());
    assert_eq!(std::fs::read_dir(scratch.path())?.count(), 0);
    Ok(())
}

#[tokio::test]
async fn escaping_symlinks_and_special_files_cannot_enter_the_scanner_tree() -> TestResult {
    let scratch = tempfile::tempdir()?;
    let source = scratch.path().join("source");
    std::fs::create_dir(&source)?;
    let sentinel = scratch.path().join("host-sentinel");
    std::fs::write(&sentinel, b"host-private")?;
    std::os::unix::fs::symlink("../../host-sentinel", source.join("escape"))?;
    let artifact = layers(image(&source, &scratch.path().join("rootfs.ext4")).await?);
    let output = scratch.path().join("scan");
    std::fs::create_dir(&output)?;
    assert!(prepare(&output, &artifact, ScanLimits::default()).is_err());
    assert_eq!(std::fs::read(&sentinel)?, b"host-private");
    std::fs::remove_file(source.join("escape"))?;
    run(
        tokio::process::Command::new("mkfifo").arg(source.join("pipe")),
        Duration::from_secs(5),
    )
    .await?;
    let artifact = layers(image(&source, &scratch.path().join("special.ext4")).await?);
    let output = scratch.path().join("special-scan");
    std::fs::create_dir(&output)?;
    assert!(prepare(&output, &artifact, ScanLimits::default()).is_err());
    Ok(())
}

#[tokio::test]
async fn firecracker_snapshot_scans_the_actual_ext4_tree_without_vmm_state_or_memory() -> TestResult {
    let scratch = tempfile::tempdir()?;
    let source = scratch.path().join("source");
    std::fs::create_dir_all(source.join("var/lib/dpkg"))?;
    std::fs::create_dir_all(source.join("etc"))?;
    std::fs::write(source.join("etc/os-release"), b"ID=debian\nVERSION_ID=12\n")?;
    std::fs::write(
        source.join("var/lib/dpkg/status"),
        b"Package: fixture\nStatus: install ok installed\nVersion: 1.0\n",
    )?;
    std::fs::hard_link(source.join("var/lib/dpkg/status"), source.join("status-copy"))?;
    let artifact = layers(image(&source, &scratch.path().join("rootfs.ext4")).await?);
    let output = scratch.path().join("scan");
    std::fs::create_dir(&output)?;
    let prepared = prepare_scan_input_async(
        &output,
        ArtifactType::FirecrackerSnapshotV1,
        &artifact,
        &preparation(ScanLimits::default()),
    )
    .await?;
    assert!(
        prepared.is_dir(),
        "Firecracker metadata, RAM and ext4 bytes are not an image tar archive"
    );
    assert_eq!(
        std::fs::read(prepared.join("var/lib/dpkg/status"))?,
        std::fs::read(source.join("var/lib/dpkg/status"))?
    );
    assert!(!prepared.join("snapshot.memory").exists());
    assert!(!prepared.join("snapshot.state").exists());
    assert_ne!(
        std::fs::metadata(prepared.join("status-copy"))?.ino(),
        std::fs::metadata(prepared.join("var/lib/dpkg/status"))?.ino(),
        "image hardlinks must not become host hardlinks"
    );
    Ok(())
}
