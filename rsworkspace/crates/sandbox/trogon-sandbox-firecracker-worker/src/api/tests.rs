use std::sync::Arc;

use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixListener;
use tokio::sync::Mutex;

use super::*;
use crate::config::{BaseRootfs, FirecrackerBinary, StateDir};

fn to_json<T: Serialize>(value: &T) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    Ok(serde_json::to_value(value)?)
}

#[test]
fn boot_source_body_matches_the_firecracker_schema() -> Result<(), Box<dyn std::error::Error>> {
    let body = BootSource::new(&KernelImage::new("/k/vmlinux"), "console=ttyS0 reboot=k panic=1");
    assert_eq!(
        to_json(&body)?,
        json!({"kernel_image_path": "/k/vmlinux", "boot_args": "console=ttyS0 reboot=k panic=1"})
    );
    Ok(())
}

#[test]
fn root_drive_body_is_a_writable_root_device() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(
        to_json(&Drive::root("/s/l1/rootfs.ext4".into()))?,
        json!({
            "drive_id": "rootfs",
            "path_on_host": "/s/l1/rootfs.ext4",
            "is_root_device": true,
            "is_read_only": false
        })
    );
    Ok(())
}

#[test]
fn machine_config_and_vsock_and_action_bodies() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(
        to_json(&MachineConfig::new(VcpuCount::new(2)?, MemoryMib::new(512)?))?,
        json!({"vcpu_count": 2, "mem_size_mib": 512})
    );
    assert_eq!(
        to_json(&VsockDevice::new(GuestCid::DEFAULT, "/s/l1/v.sock".into()))?,
        json!({"guest_cid": 3, "uds_path": "/s/l1/v.sock"})
    );
    assert_eq!(
        to_json(&Action::INSTANCE_START)?,
        json!({"action_type": "InstanceStart"})
    );
    Ok(())
}

/// Answers every request on `socket` with `status`, recording `(request line, body)`.
async fn fake_firecracker(
    socket: &std::path::Path,
    status: &'static str,
    reply_body: &'static str,
) -> Result<Arc<Mutex<Vec<(String, String)>>>, Box<dyn std::error::Error>> {
    let listener = UnixListener::bind(socket)?;
    let seen = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&seen);
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            let recorded = Arc::clone(&recorded);
            tokio::spawn(async move {
                let mut raw = Vec::new();
                let mut buf = [0u8; 1024];
                let (head, body) = loop {
                    let Ok(n) = stream.read(&mut buf).await else {
                        return;
                    };
                    if n == 0 {
                        return;
                    }
                    raw.extend_from_slice(&buf[..n]);
                    let text = String::from_utf8_lossy(&raw).to_string();
                    if let Some((head, body)) = text.split_once("\r\n\r\n") {
                        let length = head
                            .lines()
                            .find_map(|l| {
                                l.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .and_then(|v| v.trim().parse::<usize>().ok())
                            })
                            .unwrap_or(0);
                        if body.len() >= length {
                            break (head.to_string(), body.to_string());
                        }
                    }
                };
                let Some(line) = head.lines().next().map(str::to_string) else {
                    return;
                };
                recorded.lock().await.push((line, body));
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply_body}",
                    reply_body.len()
                );
                let _ = stream.write_all(response.as_bytes()).await;
            });
        }
    });
    Ok(seen)
}

#[tokio::test]
async fn the_boot_sequence_is_sent_in_firecracker_order() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let socket = tmp.path().join("api.sock");
    let seen = fake_firecracker(&socket, "204 No Content", "").await?;

    let state = StateDir::new(tmp.path());
    let config = FirecrackerConfig::new(
        FirecrackerBinary::new("firecracker"),
        KernelImage::new("/k/vmlinux"),
        BaseRootfs::new("/base.ext4"),
        state.clone(),
    );
    let dir = VmDir::for_lease(&state, &trogon_sandbox_contract::LeaseId::new("l1"))?;

    FirecrackerApi::new(socket)
        .configure_and_start(
            &config,
            MemoryMib::new(256)?,
            &trogon_sandbox_contract::LeaseId::new("l1"),
            false,
        )
        .await?;

    let seen = seen.lock().await;
    let lines: Vec<&str> = seen.iter().map(|(l, _)| l.as_str()).collect();
    assert_eq!(
        lines,
        [
            "PUT /boot-source HTTP/1.1",
            "PUT /drives/rootfs HTTP/1.1",
            "PUT /machine-config HTTP/1.1",
            "PUT /vsock HTTP/1.1",
            "PUT /actions HTTP/1.1",
        ]
    );
    let machine: serde_json::Value = serde_json::from_str(&seen[2].1)?;
    assert_eq!(machine, json!({"vcpu_count": 2, "mem_size_mib": 256}));
    let boot: serde_json::Value = serde_json::from_str(&seen[0].1)?;
    assert_eq!(boot["boot_args"], "console=ttyS0 reboot=k panic=1 ate.actor_id=l1");
    let drive: serde_json::Value = serde_json::from_str(&seen[1].1)?;
    assert_eq!(drive["path_on_host"], "rootfs.ext4");
    let vsock: serde_json::Value = serde_json::from_str(&seen[3].1)?;
    assert_eq!(vsock["uds_path"], "v.sock");
    assert_eq!(dir.as_path().join(VmDir::vsock_socket_from_vmm()), dir.vsock_socket());
    Ok(())
}

#[tokio::test]
async fn snapshot_restore_requests_clock_advancement_and_preserves_clock_refusal()
-> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let socket = tmp.path().join("api.sock");
    let seen = fake_firecracker(
        &socket,
        "400 Bad Request",
        r#"{"fault_message":"clock_realtime requested but not present in the snapshot state"}"#,
    )
    .await?;
    let error = FirecrackerApi::new(socket)
        .load_snapshot_files("snapshot.state".into(), "snapshot.memory".into())
        .await
        .err()
        .ok_or("expected an error")?;
    assert!(
        format!("{error:?}").contains("clock_realtime requested but not present"),
        "{error}"
    );
    let seen = seen.lock().await;
    let body: serde_json::Value = serde_json::from_str(&seen[0].1)?;
    assert_eq!(
        body["clock_realtime"], true,
        "every supported x86 guest restore must request host elapsed-time advancement"
    );
    Ok(())
}

#[tokio::test]
async fn snapshot_calls_match_the_firecracker_schema() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let socket = tmp.path().join("api.sock");
    let seen = fake_firecracker(&socket, "204 No Content", "").await?;
    let api = FirecrackerApi::new(socket);

    api.pause().await?;
    api.create_snapshot(&SnapshotCreate::full("/n/vmstate".into(), "/n/memory".into()))
        .await?;
    api.resume().await?;
    api.load_snapshot(&SnapshotLoad::resumed("/n/vmstate".into(), "/n/memory".into()))
        .await?;

    let seen = seen.lock().await;
    let calls = seen
        .iter()
        .map(|(line, body)| Ok((line.as_str(), serde_json::from_str::<serde_json::Value>(body)?)))
        .collect::<Result<Vec<_>, serde_json::Error>>()?;
    assert_eq!(
        calls,
        [
            ("PATCH /vm HTTP/1.1", json!({"state": "Paused"})),
            (
                "PUT /snapshot/create HTTP/1.1",
                json!({
                    "snapshot_type": "Full",
                    "snapshot_path": "/n/vmstate",
                    "mem_file_path": "/n/memory"
                })
            ),
            ("PATCH /vm HTTP/1.1", json!({"state": "Resumed"})),
            (
                "PUT /snapshot/load HTTP/1.1",
                json!({
                    "snapshot_path": "/n/vmstate",
                    "mem_backend": {"backend_type": "File", "backend_path": "/n/memory"},
                    "resume_vm": true,
                    "clock_realtime": true
                })
            ),
        ]
    );
    Ok(())
}

#[tokio::test]
async fn a_rejected_patch_names_its_method() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let socket = tmp.path().join("api.sock");
    fake_firecracker(&socket, "400 Bad Request", r#"{"fault_message":"not running"}"#).await?;

    let err = FirecrackerApi::new(socket)
        .pause()
        .await
        .err()
        .ok_or("expected an error")?;
    assert!(matches!(
        err,
        ApiError::Rejected {
            method: "PATCH",
            path: "/vm",
            status: 400,
            ..
        }
    ));
    Ok(())
}

#[tokio::test]
async fn a_networked_lease_attaches_the_tap_and_configures_the_guest_address() -> Result<(), Box<dyn std::error::Error>>
{
    let tmp = tempfile::tempdir()?;
    let socket = tmp.path().join("api.sock");
    let seen = fake_firecracker(&socket, "204 No Content", "").await?;

    let state = StateDir::new(tmp.path());
    let config = FirecrackerConfig::new(
        FirecrackerBinary::new("firecracker"),
        KernelImage::new("/k/vmlinux"),
        BaseRootfs::new("/base.ext4"),
        state.clone(),
    );
    let lease = trogon_sandbox_contract::LeaseId::new("l1");

    FirecrackerApi::new(socket)
        .configure_and_start(&config, MemoryMib::new(256)?, &lease, true)
        .await?;

    let seen = seen.lock().await;
    let lines: Vec<&str> = seen.iter().map(|(l, _)| l.as_str()).collect();
    assert_eq!(
        lines[4..],
        ["PUT /network-interfaces/eth0 HTTP/1.1", "PUT /actions HTTP/1.1"]
    );
    let nic: serde_json::Value = serde_json::from_str(&seen[4].1)?;
    assert_eq!(
        nic,
        json!({"iface_id": "eth0", "host_dev_name": "tap0", "guest_mac": "06:00:ac:10:00:02"})
    );
    let boot: serde_json::Value = serde_json::from_str(&seen[0].1)?;
    assert_eq!(
        boot["boot_args"],
        "console=ttyS0 reboot=k panic=1 ip=172.16.0.2::172.16.0.1:255.255.255.252::eth0:off ate.actor_id=l1"
    );
    Ok(())
}

#[tokio::test]
async fn a_rejection_carries_firecrackers_own_message() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let socket = tmp.path().join("api.sock");
    fake_firecracker(&socket, "400 Bad Request", r#"{"fault_message":"bad kernel"}"#).await?;

    let err = FirecrackerApi::new(socket)
        .set_machine_config(&MachineConfig {
            vcpu_count: 1,
            mem_size_mib: 1,
            cpu_template: None,
        })
        .await
        .err()
        .ok_or("expected an error")?;
    match err {
        ApiError::Rejected { status, body, .. } => {
            assert_eq!(status, 400);
            assert!(body.contains("bad kernel"));
        }
        other => return Err(format!("unexpected {other:?}").into()),
    }
    Ok(())
}

#[tokio::test]
async fn a_missing_socket_is_a_transport_error() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let err = FirecrackerApi::new(tmp.path().join("none.sock"))
        .start_instance()
        .await
        .err()
        .ok_or("expected an error")?;
    assert!(matches!(err, ApiError::Transport { .. }));
    Ok(())
}
