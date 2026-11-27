use super::*;
use trogon_sandbox_contract::CommandExitMarker;
use trogon_sandbox_contract::CommandOutputChunk;
use trogon_sandbox_contract::OutputStream;
use trogon_sandbox_contract::{FileContentHashing, FileRecursion, StdinSeq};

#[tokio::test]
async fn read_logs_long_poll_returns_when_a_chunk_arrives() -> TestResult {
    let tenant = TenantId::new("test-tenant")?;
    let lease_id = LeaseId::new("lease-under-test");
    let store = Arc::new(crate::command_logs::InMemoryCommandLogStore::new(
        crate::command_logs::CommandLogRetention::default(),
    ));
    let core = make_core(mock_provider_response()?)?.with_command_logs(store.clone());
    core.active_leases
        .put(&running_source_record(&tenant, &lease_id)?)
        .await?;

    let mut request = read_logs_request(&lease_id, "exec-1", 0);
    request.wait_ms = Some(5_000);
    let writer = {
        let lease_id = lease_id.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            store.append(
                &lease_id,
                CommandOutputChunk {
                    execution_id: ExecutionId::new("exec-1"),
                    stream: OutputStream::Stdout,
                    seq: 0,
                    data: b"late".to_vec(),
                },
            );
        })
    };
    let started = std::time::Instant::now();
    let ReadLogsResponse::Logs { chunks, .. } = core.handle_read_logs(request).await else {
        return Err("expected logs".into());
    };
    writer.await?;
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0].data, b"late");
    assert!(started.elapsed() < Duration::from_secs(4));

    // Nothing more arrives: the poll gives up after the wait, empty.
    let mut idle = read_logs_request(&lease_id, "exec-1", chunks[0].log_seq);
    idle.wait_ms = Some(100);
    let ReadLogsResponse::Logs { chunks, .. } = core.handle_read_logs(idle).await else {
        return Err("expected logs".into());
    };
    assert!(chunks.is_empty());
    Ok(())
}

#[tokio::test]
async fn read_logs_reports_an_exited_command_without_parking() -> TestResult {
    let tenant = TenantId::new("test-tenant")?;
    let lease_id = LeaseId::new("lease-under-test");
    let store = Arc::new(crate::command_logs::InMemoryCommandLogStore::new(
        crate::command_logs::CommandLogRetention::default(),
    ));
    let core = make_core(mock_provider_response()?)?.with_command_logs(store.clone());
    core.active_leases
        .put(&running_source_record(&tenant, &lease_id)?)
        .await?;
    store.mark_exited(
        &lease_id,
        &CommandExitMarker {
            execution_id: ExecutionId::new("exec-1"),
            exit_code: Some(0),
        },
    );

    let mut request = read_logs_request(&lease_id, "exec-1", 0);
    request.wait_ms = Some(5_000);
    let started = std::time::Instant::now();
    let ReadLogsResponse::Logs { chunks, exited, .. } = core.handle_read_logs(request).await else {
        return Err("expected logs".into());
    };
    assert!(chunks.is_empty());
    assert!(exited.is_reported());
    assert!(started.elapsed() < Duration::from_secs(1));
    Ok(())
}

#[tokio::test]
async fn read_logs_is_unsupported_for_a_provider_that_cannot_stream_output() -> TestResult {
    for streams in [false, true] {
        let registry = InMemoryWorkerRegistry::new(Duration::from_secs(30));
        let mut profile = default_capability_profile();
        profile.isolation.supports_output_streaming = CapabilitySupport::from(streams);
        registry
            .inject(worker_heartbeat("mock-w1", ProviderKind::Mock, profile)?)
            .await;
        let tenant = TenantId::new("test-tenant")?;
        let lease_id = LeaseId::new("lease-under-test");
        let store = Arc::new(crate::command_logs::InMemoryCommandLogStore::new(
            crate::command_logs::CommandLogRetention::default(),
        ));
        let core = make_core_with_registry(mock_provider_response()?, registry, true)?.with_command_logs(store);
        core.active_leases
            .put(&running_source_record(&tenant, &lease_id)?)
            .await?;
        let response = core.handle_read_logs(read_logs_request(&lease_id, "exec-1", 0)).await;
        if streams {
            assert!(matches!(response, ReadLogsResponse::Logs { .. }), "{response:?}");
        } else {
            assert_eq!(response, ReadLogsResponse::Unsupported);
        }
    }
    Ok(())
}

#[tokio::test]
async fn read_logs_is_unsupported_without_a_store() -> TestResult {
    let core = make_core(mock_provider_response()?)?;
    let lease_id = LeaseId::new("lease-under-test");
    assert_eq!(
        core.handle_read_logs(read_logs_request(&lease_id, "exec-1", 0)).await,
        ReadLogsResponse::Unsupported
    );
    Ok(())
}

#[tokio::test]
async fn read_logs_pages_retained_output_and_gates_on_the_lease() -> TestResult {
    let tenant = TenantId::new("test-tenant")?;
    let lease_id = LeaseId::new("lease-under-test");
    let store = Arc::new(crate::command_logs::InMemoryCommandLogStore::new(
        crate::command_logs::CommandLogRetention::default(),
    ));
    for (seq, data) in [(0u64, b"one".as_slice()), (1, b"two".as_slice())] {
        store.append(
            &lease_id,
            CommandOutputChunk {
                execution_id: ExecutionId::new("exec-1"),
                stream: OutputStream::Stdout,
                seq,
                data: data.to_vec(),
            },
        );
    }
    let core = make_core(mock_provider_response()?)?.with_command_logs(store);

    assert!(matches!(
        core.handle_read_logs(read_logs_request(&lease_id, "exec-1", 0)).await,
        ReadLogsResponse::Denied(LeaseError::LeaseNotFound { .. })
    ));

    core.active_leases
        .put(&running_source_record(&tenant, &lease_id)?)
        .await?;
    let ReadLogsResponse::Logs {
        chunks,
        next_seq,
        has_more,
        truncated,
        ..
    } = core.handle_read_logs(read_logs_request(&lease_id, "exec-1", 0)).await
    else {
        return Err("expected logs".into());
    };
    assert_eq!(chunks.len(), 2);
    assert!(!has_more.has_more());
    assert!(!truncated.is_truncated());

    let ReadLogsResponse::Logs { chunks, .. } = core
        .handle_read_logs(read_logs_request(&lease_id, "exec-1", next_seq))
        .await
    else {
        return Err("expected logs".into());
    };
    assert!(chunks.is_empty());

    assert!(matches!(
        core.handle_read_logs(read_logs_request(&lease_id, "bad.id", 0)).await,
        ReadLogsResponse::Denied(LeaseError::Backend { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn write_stdin_denied_when_lease_not_found() -> TestResult {
    let core = make_core(mock_provider_response()?)?;

    let response = core
        .handle_write_stdin(write_stdin_request(&LeaseId::new("missing"), b"x", false)?)
        .await;

    assert!(matches!(
        response,
        WriteStdinResponse::Denied(LeaseError::LeaseNotFound { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn write_stdin_denied_when_lease_not_running() -> TestResult {
    let tenant = TenantId::new("test-tenant")?;
    let lease_id = LeaseId::new("lease-under-test");
    let core = make_core(mock_provider_response()?)?;
    let mut record = running_source_record(&tenant, &lease_id)?;
    record.status = LeaseStatus::Released;
    core.active_leases.put(&record).await?;

    let response = core
        .handle_write_stdin(write_stdin_request(&lease_id, b"x", false)?)
        .await;

    assert!(matches!(
        response,
        WriteStdinResponse::Denied(LeaseError::LeaseNotRunning { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn write_stdin_refuses_oversized_and_empty_without_eof_before_dispatch() -> TestResult {
    let tenant = TenantId::new("test-tenant")?;
    let lease_id = LeaseId::new("lease-under-test");
    let bus = Arc::new(CapturingBus::new(Bytes::from(serde_json::to_vec(
        &WriteStdinResponse::Written {
            next_seq: StdinSeq::new(1),
        },
    )?)));
    let core = make_core_with_bus(bus.clone())?;
    core.active_leases
        .put(&running_source_record(&tenant, &lease_id)?)
        .await?;

    let oversized = vec![0u8; trogon_sandbox_contract::StdinChunk::MAX_LEN + 1];
    for request in [
        write_stdin_request(&lease_id, &oversized, false),
        write_stdin_request(&lease_id, b"", false),
    ] {
        assert!(request.is_err());
    }
    assert!(locked(&bus.last_payload)?.is_none());
    Ok(())
}

#[tokio::test]
async fn write_files_larger_than_one_bus_message_reach_the_chunk_transport() -> TestResult {
    let tenant = TenantId::new("test-tenant")?;
    let lease_id = LeaseId::new("lease-under-test");
    let bus =
        Arc::new(CapturingBus::new(Bytes::from(serde_json::to_vec(&WriteFilesResponse::Written)?)).limited_to(1024));
    let core = make_core_with_bus(bus.clone())?;
    core.active_leases
        .put(&running_source_record(&tenant, &lease_id)?)
        .await?;

    let response = core
        .handle_write_files(WriteFilesRequest {
            lease_id,
            files: vec![trogon_sandbox_contract::SandboxFileWrite {
                path: "/workspace/big".to_string(),
                content: vec![b'a'; 2048],
                mode: None,
            }],
            as_user: None,
        })
        .await;

    assert_eq!(response, WriteFilesResponse::Written);
    let captured = locked(&bus.last_payload)?;
    let request: WriteFilesRequest = serde_json::from_slice(captured.as_ref().ok_or("no payload was dispatched")?)?;
    assert_eq!(request.files[0].content, vec![b'a'; 2048]);
    Ok(())
}

#[tokio::test]
async fn file_op_forwards_the_request_and_returns_the_typed_reply() -> TestResult {
    let tenant = TenantId::new("test-tenant")?;
    let lease_id = LeaseId::new("lease-under-test");
    let expected = FileOpResponse::Edited {
        occurrences: 2,
        size: 11,
    };
    let bus = Arc::new(CapturingBus::new(Bytes::from(serde_json::to_vec(&expected)?)));
    let core = make_core_with_bus(bus.clone())?;
    core.active_leases
        .put(&running_source_record(&tenant, &lease_id)?)
        .await?;
    let op = trogon_sandbox_contract::FileOp::Edit {
        path: "/workspace/a.txt".to_string(),
        find: "foo".to_string(),
        replace: "bar".to_string(),
        expected_occurrences: 2,
    };

    let response = core
        .handle_file_op(FileOpRequest {
            lease_id,
            op: op.clone(),
            as_user: None,
        })
        .await;

    assert_eq!(response, expected);
    let captured = locked(&bus.last_payload)?;
    let request: FileOpRequest = serde_json::from_slice(captured.as_ref().ok_or("no payload was dispatched")?)?;
    assert_eq!(request.op, op);
    Ok(())
}

#[tokio::test]
async fn file_op_passes_through_a_typed_unsupported_reply() -> TestResult {
    let tenant = TenantId::new("test-tenant")?;
    let lease_id = LeaseId::new("lease-under-test");
    let op = trogon_sandbox_contract::FileOp::Stat {
        path: "/workspace".to_string(),
    };
    let unsupported = FileOpResponse::Unsupported {
        message: "mock does not serve stat".to_string(),
    };
    let core = make_core(Bytes::from(serde_json::to_vec(&unsupported)?))?;
    core.active_leases
        .put(&running_source_record(&tenant, &lease_id)?)
        .await?;

    let response = core
        .handle_file_op(FileOpRequest {
            lease_id,
            op,
            as_user: None,
        })
        .await;

    assert_eq!(response, unsupported);
    Ok(())
}

#[tokio::test]
async fn file_op_is_denied_for_unknown_and_stopped_leases_without_dispatch() -> TestResult {
    let tenant = TenantId::new("test-tenant")?;
    let bus = Arc::new(CapturingBus::new(Bytes::new()));
    let core = make_core_with_bus(bus.clone())?;
    let op = trogon_sandbox_contract::FileOp::Stat {
        path: "/workspace".to_string(),
    };

    let missing = LeaseId::new("missing");
    let response = core
        .handle_file_op(FileOpRequest {
            lease_id: missing.clone(),
            op: op.clone(),
            as_user: None,
        })
        .await;
    assert_eq!(
        response,
        FileOpResponse::Denied(LeaseError::LeaseNotFound { lease_id: missing })
    );

    let stopped = LeaseId::new("stopped");
    let mut record = running_source_record(&tenant, &stopped)?;
    record.status = LeaseStatus::Cancelling;
    core.active_leases.put(&record).await?;
    let response = core
        .handle_file_op(FileOpRequest {
            lease_id: stopped.clone(),
            op,
            as_user: None,
        })
        .await;
    assert_eq!(
        response,
        FileOpResponse::Denied(LeaseError::LeaseNotRunning { lease_id: stopped })
    );
    assert!(locked(&bus.last_payload)?.is_none());
    Ok(())
}

#[tokio::test]
async fn tracking_and_watch_file_ops_are_refused_when_the_profile_lacks_the_capability() -> TestResult {
    let tenant = TenantId::new("test-tenant")?;
    let lease_id = LeaseId::new("lease-under-test");
    let bus = Arc::new(CapturingBus::new(Bytes::new()));
    let mut core = make_core_with_bus(bus.clone())?;
    core.worker_registry = registry_with_tracking(false, false).await?;
    core.active_leases
        .put(&running_source_record(&tenant, &lease_id)?)
        .await?;

    for op in [
        trogon_sandbox_contract::FileOp::Checkpoint {
            path: "/workspace".to_string(),
            hash: FileContentHashing::Metadata,
        },
        trogon_sandbox_contract::FileOp::Changes {
            since: None,
            path: None,
        },
        trogon_sandbox_contract::FileOp::WatchStart {
            path: "/workspace".to_string(),
            recursive: FileRecursion::from(true),
        },
        trogon_sandbox_contract::FileOp::WatchPoll {
            watch_id: "w-1".to_string(),
            after_seq: 0,
            wait_ms: None,
        },
    ] {
        let response = core
            .handle_file_op(FileOpRequest {
                lease_id: lease_id.clone(),
                op,
                as_user: None,
            })
            .await;
        assert!(matches!(response, FileOpResponse::Unsupported { .. }), "{response:?}");
    }
    assert!(locked(&bus.last_payload)?.is_none());
    Ok(())
}

#[tokio::test]
async fn each_tracking_family_is_gated_by_its_own_capability_and_forwarded_when_present() -> TestResult {
    let tenant = TenantId::new("test-tenant")?;
    let lease_id = LeaseId::new("lease-under-test");
    let expected = FileOpResponse::WatchStarted {
        watch_id: "w-1".to_string(),
    };
    let bus = Arc::new(CapturingBus::new(Bytes::from(serde_json::to_vec(&expected)?)));
    let mut core = make_core_with_bus(bus.clone())?;
    core.worker_registry = registry_with_tracking(false, true).await?;
    core.active_leases
        .put(&running_source_record(&tenant, &lease_id)?)
        .await?;

    let checkpoint = core
        .handle_file_op(FileOpRequest {
            lease_id: lease_id.clone(),
            op: trogon_sandbox_contract::FileOp::Checkpoint {
                path: "/workspace".to_string(),
                hash: FileContentHashing::Metadata,
            },
            as_user: None,
        })
        .await;
    assert!(matches!(checkpoint, FileOpResponse::Unsupported { .. }));
    assert!(locked(&bus.last_payload)?.is_none());

    let watch = core
        .handle_file_op(FileOpRequest {
            lease_id,
            op: trogon_sandbox_contract::FileOp::WatchStart {
                path: "/workspace".to_string(),
                recursive: FileRecursion::from(false),
            },
            as_user: None,
        })
        .await;
    assert_eq!(watch, expected);
    assert!(locked(&bus.last_payload)?.is_some());
    Ok(())
}

#[tokio::test]
async fn checksum_file_ops_follow_the_dir_transfer_capability() -> TestResult {
    let tenant = TenantId::new("test-tenant")?;
    let lease_id = LeaseId::new("lease-under-test");
    let expected = FileOpResponse::Checksum {
        sha256: trogon_sandbox_contract::FileSha256::new("00".repeat(32))?,
        bytes: 3,
        total_size: 3,
    };
    let checksum = || FileOpRequest {
        lease_id: lease_id.clone(),
        op: trogon_sandbox_contract::FileOp::Checksum {
            path: "/workspace/f".to_string(),
            offset: 0,
            length: None,
        },
        as_user: None,
    };

    let bus = Arc::new(CapturingBus::new(Bytes::new()));
    let mut core = make_core_with_bus(bus.clone())?;
    core.worker_registry =
        registry_with_isolation(|i| i.supports_dir_transfer = CapabilitySupport::from(false)).await?;
    core.active_leases
        .put(&running_source_record(&tenant, &lease_id)?)
        .await?;
    let refused = core.handle_file_op(checksum()).await;
    assert!(matches!(refused, FileOpResponse::Unsupported { .. }));
    assert!(locked(&bus.last_payload)?.is_none());

    let bus = Arc::new(CapturingBus::new(Bytes::from(serde_json::to_vec(&expected)?)));
    let mut core = make_core_with_bus(bus.clone())?;
    core.worker_registry = registry_with_isolation(|i| i.supports_dir_transfer = CapabilitySupport::from(true)).await?;
    core.active_leases
        .put(&running_source_record(&tenant, &lease_id)?)
        .await?;
    assert_eq!(core.handle_file_op(checksum()).await, expected);
    assert!(locked(&bus.last_payload)?.is_some());
    Ok(())
}

#[tokio::test]
async fn write_stdin_forwards_to_provider_and_returns_response() -> TestResult {
    let tenant = TenantId::new("test-tenant")?;
    let lease_id = LeaseId::new("lease-under-test");
    let expected = WriteStdinResponse::Written {
        next_seq: StdinSeq::new(1),
    };
    let core = make_core(Bytes::from(serde_json::to_vec(&expected)?))?;
    core.active_leases
        .put(&running_source_record(&tenant, &lease_id)?)
        .await?;

    let response = core
        .handle_write_stdin(write_stdin_request(&lease_id, b"hi", false)?)
        .await;

    assert_eq!(response, expected);
    Ok(())
}

#[tokio::test]
async fn write_stdin_accepts_eof_without_data() -> TestResult {
    let tenant = TenantId::new("test-tenant")?;
    let lease_id = LeaseId::new("lease-under-test");
    let expected = WriteStdinResponse::Written {
        next_seq: StdinSeq::new(1),
    };
    let core = make_core(Bytes::from(serde_json::to_vec(&expected)?))?;
    core.active_leases
        .put(&running_source_record(&tenant, &lease_id)?)
        .await?;

    let response = core
        .handle_write_stdin(write_stdin_request(&lease_id, b"", true)?)
        .await;

    assert_eq!(response, expected);
    Ok(())
}

#[tokio::test]
async fn write_stdin_is_unsupported_when_no_worker_answers() -> TestResult {
    let tenant = TenantId::new("test-tenant")?;
    let lease_id = LeaseId::new("lease-under-test");
    let core = make_core_with_bus(Arc::new(NoRespondersBus))?;
    core.active_leases
        .put(&running_source_record(&tenant, &lease_id)?)
        .await?;

    let response = core
        .handle_write_stdin(write_stdin_request(&lease_id, b"x", false)?)
        .await;

    assert_eq!(response, WriteStdinResponse::Unsupported);
    Ok(())
}

#[tokio::test]
async fn write_stdin_maps_a_dispatch_timeout_to_backpressure_on_the_same_seq() -> TestResult {
    let tenant = TenantId::new("test-tenant")?;
    let lease_id = LeaseId::new("lease-under-test");
    let core = make_core_with_bus(Arc::new(TimedOutBus))?;
    core.active_leases
        .put(&running_source_record(&tenant, &lease_id)?)
        .await?;

    let response = core
        .handle_write_stdin(write_stdin_request(&lease_id, b"x", false)?)
        .await;

    assert_eq!(
        response,
        WriteStdinResponse::Backpressure {
            next_seq: StdinSeq::new(0)
        }
    );
    Ok(())
}

#[tokio::test]
async fn lease_tenant_returns_the_owning_tenant() -> TestResult {
    let tenant = TenantId::new("test-tenant")?;
    let lease_id = LeaseId::new("lease-under-test");
    let core = make_core(mock_provider_response()?)?;
    assert_eq!(core.lease_tenant(&lease_id).await, None);
    core.active_leases
        .put(&running_source_record(&tenant, &lease_id)?)
        .await?;
    assert_eq!(core.lease_tenant(&lease_id).await, Some(tenant));
    Ok(())
}
