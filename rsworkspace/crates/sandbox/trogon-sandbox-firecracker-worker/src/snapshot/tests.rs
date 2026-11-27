type TestResult = Result<(), Box<dyn std::error::Error>>;
use super::*;
use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use trogon_sandbox_contract::LegacySnapshotPolicy;
use trogon_sandbox_protocol::EnvironmentPolicy;

#[tokio::test]
async fn cancelled_capture_never_resumes_a_replacement_directory_generation() -> TestResult {
    let root = tempfile::tempdir()?;
    let vm = VmDir::for_lease(
        &crate::config::StateDir::new(root.path()),
        &trogon_sandbox_contract::LeaseId::new("reused-capture"),
    )?;
    vm.create().await?;
    tokio::fs::write(vm.rootfs(), b"rootfs").await?;
    let entered = Arc::new(tokio::sync::Notify::new());
    let notify = Arc::clone(&entered);
    let original = tokio::net::UnixListener::bind(vm.api_socket())?;
    let app = axum::Router::new()
        .route(
            "/snapshot/create",
            axum::routing::put(move || {
                let notify = Arc::clone(&notify);
                async move {
                    notify.notify_one();
                    std::future::pending::<()>().await;
                    axum::http::StatusCode::NO_CONTENT
                }
            }),
        )
        .fallback(axum::routing::any(|| async { axum::http::StatusCode::NO_CONTENT }));
    let old_server = tokio::spawn(async move {
        let _ = axum::serve(original, app).await;
    });
    let identity =
        trogon_sandbox_contract::LeaseId::new(std::fs::canonicalize(vm.api_socket())?.to_string_lossy().into_owned());
    let capture_dir = vm.clone();
    let capture = tokio::spawn(async move {
        LocalSnapshot::capture(
            &capture_dir,
            SnapshotMetadata::new(MemoryMib::new(1)?, VcpuCount::new(2)?),
        )
        .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(2), entered.notified()).await?;
    tokio::fs::rename(vm.as_path(), root.path().join("retired-generation")).await?;
    vm.create().await?;
    let replacement = tokio::net::UnixListener::bind(vm.api_socket())?;
    let requests = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&requests);
    let app = axum::Router::new().fallback(axum::routing::any(move || {
        let observed = Arc::clone(&observed);
        async move {
            observed.fetch_add(1, Ordering::SeqCst);
            axum::http::StatusCode::NO_CONTENT
        }
    }));
    let new_server = tokio::spawn(async move {
        let _ = axum::serve(replacement, app).await;
    });
    capture.abort();
    assert!(matches!(capture.await, Err(error) if error.is_cancelled()));
    let complete = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        CAPTURE_GATES
            .get()
            .ok_or("the capture gates must exist")?
            .lock(&identity),
    )
    .await?;
    drop(complete);
    old_server.abort();
    new_server.abort();
    assert_eq!(
        requests.load(Ordering::SeqCst),
        0,
        "old recovery targeted the replacement VM socket"
    );
    Ok(())
}

#[tokio::test]
async fn cancelling_capture_after_pause_resumes_the_source_vm() -> TestResult {
    let root = tempfile::tempdir()?;
    let vm = VmDir::for_lease(
        &crate::config::StateDir::new(root.path()),
        &trogon_sandbox_contract::LeaseId::new("cancel-capture"),
    )?;
    vm.create().await?;
    tokio::fs::write(vm.rootfs(), b"rootfs").await?;
    let paused = Arc::new(AtomicBool::new(false));
    let entered = Arc::new(tokio::sync::Notify::new());
    let resumed = Arc::new(tokio::sync::Notify::new());
    let listener = tokio::net::UnixListener::bind(vm.api_socket())?;
    let state = (paused.clone(), entered.clone(), resumed.clone());
    let app = axum::Router::new()
        .route(
            "/vm",
            axum::routing::patch(
                |axum::extract::State((paused, _, resumed)): axum::extract::State<(
                    Arc<AtomicBool>,
                    Arc<tokio::sync::Notify>,
                    Arc<tokio::sync::Notify>,
                )>,
                 axum::Json(body): axum::Json<serde_json::Value>| async move {
                    let value = body["state"] == "Paused";
                    paused.store(value, Ordering::SeqCst);
                    if !value {
                        resumed.notify_one();
                    }
                    axum::http::StatusCode::NO_CONTENT
                },
            ),
        )
        .route(
            "/snapshot/create",
            axum::routing::put(
                |axum::extract::State((_, entered, _)): axum::extract::State<(
                    Arc<AtomicBool>,
                    Arc<tokio::sync::Notify>,
                    Arc<tokio::sync::Notify>,
                )>| async move {
                    entered.notify_one();
                    std::future::pending::<()>().await;
                    axum::http::StatusCode::NO_CONTENT
                },
            ),
        )
        .with_state(state);
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    let task = tokio::spawn(async move {
        LocalSnapshot::capture(&vm, SnapshotMetadata::new(MemoryMib::new(1)?, VcpuCount::new(2)?)).await
    });
    tokio::time::timeout(std::time::Duration::from_secs(2), entered.notified()).await?;
    assert!(paused.load(Ordering::SeqCst));
    task.abort();
    assert!(matches!(task.await, Err(error) if error.is_cancelled()));
    let result = tokio::time::timeout(std::time::Duration::from_secs(2), resumed.notified()).await;
    server.abort();
    assert!(result.is_ok(), "cancelled capture left its source VM paused");
    assert!(!paused.load(Ordering::SeqCst));
    Ok(())
}

#[tokio::test]
async fn cancelled_materialization_keeps_its_reservation_until_the_actual_writer_stops() -> TestResult {
    struct GatedRead {
        data: std::io::Cursor<Vec<u8>>,
        entered: Option<tokio::sync::oneshot::Sender<()>>,
        gate: std::sync::Arc<std::sync::Barrier>,
    }
    impl std::io::Read for GatedRead {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            if let Some(entered) = self.entered.take() {
                entered
                    .send(())
                    .map_err(|()| std::io::Error::other("the observer is gone"))?;
                self.gate.wait();
            }
            std::io::Read::read(&mut self.data, buffer)
        }
    }
    let destination = tempfile::tempdir()?;
    let path = destination.path().join("memory");
    let written = path.clone();
    let reservation = crate::space::reserve(destination.path(), 1024)?;
    let weak = std::sync::Arc::downgrade(&reservation);
    let (entered, ready) = tokio::sync::oneshot::channel();
    let (finished, done) = tokio::sync::oneshot::channel();
    let gate = std::sync::Arc::new(std::sync::Barrier::new(2));
    let writer_gate = std::sync::Arc::clone(&gate);
    let task = tokio::spawn(async move {
        let writing_space = std::sync::Arc::clone(&reservation);
        let _ = tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
            let mut reader = GatedRead {
                data: std::io::Cursor::new(b"actual snapshot writer".to_vec()),
                entered: Some(entered),
                gate: writer_gate,
            };
            materialize_file(
                &mut reader,
                &written,
                22,
                true,
                std::time::Instant::now() + std::time::Duration::from_secs(10),
                writing_space,
            )?;
            let _ = finished.send(());
            Ok(())
        })
        .await;
        drop(reservation);
    });
    ready.await?;
    task.abort();
    assert!(task.await.is_err_and(|error| error.is_cancelled()));
    assert!(weak.upgrade().is_some());
    gate.wait();
    done.await?;
    assert!(weak.upgrade().is_none());
    assert_eq!(std::fs::read(path)?, b"actual snapshot writer");
    Ok(())
}

#[tokio::test]
async fn restore_rejects_oversized_metadata_before_creating_guest_files() -> TestResult {
    let source = tempfile::tempdir()?;
    let snapshot = LocalSnapshot::fixture(source.path(), 1)?;
    let mut layers = snapshot.layers().await?;
    let metadata = layers
        .iter_mut()
        .find(|layer| layer.annotations.get(ROLE).is_some_and(|role| role == METADATA_FILE))
        .ok_or("the metadata layer must exist")?;
    metadata.data.resize(3 * 1024 * 1024, b' ');
    let destination = tempfile::tempdir()?;
    assert!(restore_layers(&layers, destination.path()).await.is_err());
    assert_eq!(std::fs::read_dir(destination.path())?.count(), 0);
    Ok(())
}

#[tokio::test]
async fn restored_machine_mismatch_is_refused_before_materializing_disk_or_ram() -> TestResult {
    let source = tempfile::tempdir()?;
    let snapshot = LocalSnapshot::fixture(source.path(), 1)?;
    let layers = snapshot.layers().await?;
    let destination = tempfile::tempdir()?;
    assert!(
        restore_layers_validated(
            &layers,
            destination.path(),
            Some(RestoreMachine {
                memory: MemoryMib::new(2)?,
                vcpus: VcpuCount::new(2)?,
                rootfs_max_bytes: MAX_ROOTFS_BYTES,
            }),
            None,
        )
        .await
        .is_err()
    );
    assert_eq!(std::fs::read_dir(destination.path())?.count(), 0);
    Ok(())
}

#[tokio::test]
async fn export_compression_stops_at_the_shared_transfer_budget() -> TestResult {
    let source = tempfile::tempdir()?;
    let snapshot = LocalSnapshot::fixture(source.path(), 1)?;
    assert!(layers_with_budget(snapshot.path(), 512).await.is_err());
    assert!(layers_with_budget(snapshot.path(), 16384).await.is_ok());
    Ok(())
}

#[tokio::test]
async fn full_memory_and_root_disk_lengths_are_exact_and_operator_budgeted() -> TestResult {
    let source = tempfile::tempdir()?;
    let snapshot = LocalSnapshot::fixture(source.path(), 1)?;
    let original = snapshot.layers().await?;
    for length in [1024 * 1024 - 1, 1024 * 1024 + 1] {
        let mut layers = original.clone();
        let memory = layers
            .iter_mut()
            .find(|layer| layer.annotations[ROLE] == MEMORY_FILE)
            .ok_or("the layer must exist")?;
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        encoder.write_all(&vec![0; length])?;
        memory.data = encoder.finish()?;
        let destination = tempfile::tempdir()?;
        assert!(restore_layers(&layers, destination.path()).await.is_err());
        assert_eq!(std::fs::read_dir(destination.path())?.count(), 0);
    }
    let destination = tempfile::tempdir()?;
    assert!(
        restore_layers_validated(
            &original,
            destination.path(),
            Some(RestoreMachine {
                memory: MemoryMib::new(1)?,
                vcpus: VcpuCount::new(2)?,
                rootfs_max_bytes: 5,
            }),
            None,
        )
        .await
        .is_err()
    );
    assert_eq!(std::fs::read_dir(destination.path())?.count(), 0);
    let mut layers = original;
    let rootfs = layers
        .iter_mut()
        .find(|layer| layer.annotations[ROLE] == ROOTFS_FILE)
        .ok_or("the layer must exist")?;
    rootfs.media_type = "application/octet-stream".into();
    rootfs.data = b"rootfs-extra".to_vec();
    assert!(restore_layers(&layers, destination.path()).await.is_err());
    assert_eq!(std::fs::read_dir(destination.path())?.count(), 0);
    Ok(())
}

#[test]
fn restore_preserves_device_presence_and_can_restrict_saved_egress() -> TestResult {
    assert_eq!(
        restore_network_policy(Some(&NetworkPolicy::AllowAll), None)?,
        Some(NetworkPolicy::AllowAll)
    );
    assert_eq!(
        restore_network_policy(Some(&NetworkPolicy::AllowAll), Some(&NetworkPolicy::DenyAll))?,
        Some(NetworkPolicy::DenyAll)
    );
    assert_eq!(restore_network_policy(None, Some(&NetworkPolicy::DenyAll))?, None);
    assert!(restore_network_policy(None, Some(&NetworkPolicy::AllowAll)).is_err());
    Ok(())
}

#[tokio::test]
async fn failed_capture_resumes_the_live_vm_and_removes_temporary_files() -> TestResult {
    let state = tempfile::tempdir()?;
    let vm = VmDir::for_lease(
        &crate::config::StateDir::new(state.path()),
        &trogon_sandbox_contract::LeaseId::new("lease"),
    )?;
    vm.create().await?;
    tokio::fs::write(vm.rootfs(), b"rootfs").await?;
    let listener = tokio::net::UnixListener::bind(vm.api_socket())?;
    let server = tokio::spawn(async move {
        let mut seen = Vec::new();
        for status in ["204 No Content", "400 Bad Request", "204 No Content"] {
            let (mut stream, _) = listener.accept().await?;
            let mut raw = Vec::new();
            loop {
                let mut buf = [0u8; 1024];
                let n = stream.read(&mut buf).await?;
                anyhow::ensure!(n != 0, "the client closed before sending a request");
                raw.extend_from_slice(&buf[..n]);
                if raw.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                    break;
                }
            }
            seen.push(
                String::from_utf8_lossy(&raw)
                    .lines()
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("the request has no line"))?
                    .to_string(),
            );
            stream
                .write_all(format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").as_bytes())
                .await?;
        }
        anyhow::Ok(seen)
    });
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        LocalSnapshot::capture(&vm, SnapshotMetadata::new(MemoryMib::new(1)?, VcpuCount::new(2)?)),
    )
    .await?;
    assert!(result.is_err());
    assert_eq!(
        tokio::time::timeout(std::time::Duration::from_secs(2), server).await???,
        [
            "PATCH /vm HTTP/1.1",
            "PUT /snapshot/create HTTP/1.1",
            "PATCH /vm HTTP/1.1"
        ]
    );
    let mut remaining = std::fs::read_dir(vm.runtime_dir())?
        .map(|entry| entry.map(|entry| entry.file_name()))
        .collect::<Result<Vec<_>, _>>()?;
    remaining.sort();
    assert_eq!(
        remaining,
        [
            std::ffi::OsString::from("api.sock"),
            std::ffi::OsString::from(ROOTFS_FILE)
        ]
    );
    assert_eq!(std::fs::read_dir(state.path())?.count(), 1);
    Ok(())
}

fn layers() -> anyhow::Result<Vec<Layer>> {
    let metadata = SnapshotMetadata::new(MemoryMib::new(1)?, VcpuCount::new(2)?);
    Ok([
        (METADATA_FILE, serde_json::to_vec(&metadata)?),
        (STATE_FILE, b"vm-state".to_vec()),
        (MEMORY_FILE, vec![42; 1024 * 1024]),
        (ROOTFS_FILE, b"guest-disk".to_vec()),
    ]
    .into_iter()
    .map(|(file, data)| Layer {
        data,
        media_type: String::new(),
        annotations: [(ROLE.into(), file.into())].into_iter().collect(),
    })
    .collect())
}

fn env(c: char) -> anyhow::Result<trogon_sandbox_contract::EnvDigest> {
    Ok(trogon_sandbox_contract::EnvDigest::new(format!(
        "sha256:{}",
        c.to_string().repeat(64)
    ))?)
}

fn layers_with_env(digest: Option<trogon_sandbox_contract::EnvDigest>) -> anyhow::Result<Vec<Layer>> {
    let mut layers = layers()?;
    let metadata = SnapshotMetadata::new(MemoryMib::new(1)?, VcpuCount::new(2)?).with_env_digest(digest);
    layers[0].data = serde_json::to_vec(&metadata)?;
    Ok(layers)
}

async fn restore_under(
    layers: &[Layer],
    policy: &trogon_sandbox_protocol::EnvironmentPolicy,
    manifest_label: Option<trogon_sandbox_contract::EnvDigest>,
) -> anyhow::Result<(anyhow::Result<RestoredSnapshot>, usize)> {
    let destination = tempfile::tempdir()?;
    let machine = RestoreMachine {
        memory: MemoryMib::new(1)?,
        vcpus: VcpuCount::new(2)?,
        rootfs_max_bytes: MAX_ROOTFS_BYTES,
    };
    let result = restore_layers_validated(
        layers,
        destination.path(),
        Some(machine),
        Some(&RestoreEnvironment { policy, manifest_label }),
    )
    .await;
    Ok((result, std::fs::read_dir(destination.path())?.count()))
}

#[tokio::test]
async fn restore_under_another_environment_is_refused_before_writing_anything() -> TestResult {
    let policy = EnvironmentPolicy::new(env('a')?);

    let (result, written) = restore_under(&layers_with_env(Some(env('b')?))?, &policy, None).await?;
    assert!(
        result
            .err()
            .ok_or("the restore must fail")?
            .to_string()
            .contains("snapshot was taken in environment")
    );
    assert_eq!(written, 0);

    let (result, written) = restore_under(&layers_with_env(Some(env('a')?))?, &policy, None).await?;
    result?;
    assert!(written > 0);

    // The manifest annotation must agree with the metadata inside the artifact.
    let (result, written) = restore_under(&layers_with_env(Some(env('a')?))?, &policy, Some(env('b')?)).await?;
    assert!(
        result
            .err()
            .ok_or("the restore must fail")?
            .to_string()
            .contains("disagree")
    );
    assert_eq!(written, 0);

    // Unlabeled snapshots: allowed by default, refused once the policy is strict.
    let (result, _) = restore_under(&layers_with_env(None)?, &policy, None).await?;
    result?;
    let strict = policy.clone().with_legacy_snapshots(LegacySnapshotPolicy::Refuse);
    let (result, written) = restore_under(&layers_with_env(None)?, &strict, None).await?;
    assert!(result.is_err());
    assert_eq!(written, 0);
    Ok(())
}

#[test]
fn old_metadata_without_an_env_digest_still_parses_and_the_digest_round_trips() -> TestResult {
    let old = r#"{"format_version":1,"architecture":"x86_64","memory_mib":1,"vcpus":2}"#;
    let parsed: SnapshotMetadata = serde_json::from_str(old)?;
    assert_eq!(parsed.env_digest, None);
    assert!(!serde_json::to_string(&parsed)?.contains("env_digest"));
    let labeled = parsed.with_env_digest(Some(env('c')?));
    let again: SnapshotMetadata = serde_json::from_str(&serde_json::to_string(&labeled)?)?;
    assert_eq!(again.env_digest, Some(env('c')?));
    assert_eq!(
        manifest_annotations(&labeled).get(trogon_sandbox_contract::EnvDigest::ANNOTATION_KEY),
        Some(&env('c')?.to_string())
    );
    assert!(manifest_annotations(&SnapshotMetadata::new(MemoryMib::new(1)?, VcpuCount::new(2)?)).is_empty());
    Ok(())
}

#[tokio::test]
async fn restore_materializes_only_the_validated_snapshot_files() -> TestResult {
    let dir = tempfile::tempdir()?;
    let metadata = restore_layers(&layers()?, dir.path()).await?;
    for file in [METADATA_FILE, STATE_FILE, MEMORY_FILE, ROOTFS_FILE] {
        assert_eq!(
            std::fs::metadata(dir.path().join(file))?.permissions().mode() & 0o777,
            0o600,
            "restored guest state must remain private to its owner"
        );
    }
    assert_eq!(metadata.memory_mib, 1);
    assert_eq!(tokio::fs::read(dir.path().join(ROOTFS_FILE)).await?, b"guest-disk");
    assert_eq!(
        tokio::fs::metadata(dir.path().join(MEMORY_FILE)).await?.len(),
        1024 * 1024
    );
    Ok(())
}

#[tokio::test]
async fn compressed_layers_roundtrip_without_transmitting_zero_filled_ram() -> TestResult {
    let parent = tempfile::tempdir()?;
    let snapshot = LocalSnapshot::fixture(parent.path(), 1)?;
    let layers = snapshot.layers().await?;
    let memory = layers
        .iter()
        .find(|layer| layer.annotations[ROLE] == MEMORY_FILE)
        .ok_or("the layer must exist")?;
    assert!(memory.media_type.ends_with("+gzip"));
    assert!(memory.data.len() < 16 * 1024);
    let restored = tempfile::tempdir()?;
    restore_layers(&layers, restored.path()).await?;
    #[cfg(unix)]
    {
        let stat = std::fs::metadata(restored.path().join(MEMORY_FILE))?;
        assert!(stat.blocks() * 512 < 64 * 1024);
    }
    assert_eq!(std::fs::metadata(restored.path().join(MEMORY_FILE))?.len(), 1024 * 1024);
    assert_eq!(std::fs::read(restored.path().join(ROOTFS_FILE))?, b"rootfs");
    Ok(())
}

#[tokio::test]
async fn malformed_snapshots_write_nothing() -> TestResult {
    for bad in [
        {
            let mut l = layers()?;
            l[0].annotations.insert(ROLE.into(), "../escape".into());
            l
        },
        {
            let mut l = layers()?;
            l[0].annotations.insert(ROLE.into(), ROOTFS_FILE.into());
            l
        },
        {
            let mut l = layers()?;
            l[2].data.truncate(1);
            l
        },
        {
            let mut l = layers()?;
            l.remove(0);
            l
        },
    ] {
        let dir = tempfile::tempdir()?;
        assert!(restore_layers(&bad, dir.path()).await.is_err());
        assert_eq!(std::fs::read_dir(dir.path())?.count(), 0);
    }
    Ok(())
}
