use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use trogon_sandbox_contract::{LeaseId, LifecycleEvent, OutputStream, ProviderKind, SandboxId, SandboxPhase};
use trogon_sandbox_protocol::{
    LifecycleEmitter, NoopLifecycleEmitter, PhaseTimer, lifecycle_subject, phases_supported_by, run_phase,
    sandbox_phase_label,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[derive(Default)]
struct Recorder(Mutex<Vec<LifecycleEvent>>);

#[async_trait]
impl LifecycleEmitter for Recorder {
    async fn emit(&self, event: LifecycleEvent) {
        if let Ok(mut events) = self.0.lock() {
            events.push(event);
        }
    }
}

impl Recorder {
    fn events(&self) -> Result<Vec<LifecycleEvent>, Box<dyn std::error::Error>> {
        Ok(self.0.lock().map_err(|_| "recorder poisoned")?.clone())
    }
}

#[test]
fn coarse_events_keep_the_status_tag_shape() -> TestResult {
    let event = LifecycleEvent::Started {
        sandbox_id: SandboxId::new("sb-1")?,
    };
    let json = serde_json::to_string(&event)?;
    assert!(json.contains(r#""status":"started""#));
    assert!(json.contains(r#""sandbox_id":"sb-1""#));
    Ok(())
}

#[test]
fn phase_and_output_events_round_trip() -> TestResult {
    for event in [
        LifecycleEvent::PhaseCompleted {
            phase: SandboxPhase::BootRuntime,
            duration_ms: 1100,
            detail: Some("node26".to_string()),
        },
        LifecycleEvent::Output {
            stream: OutputStream::Stdout,
            text: "fetching dataset\n".to_string(),
        },
    ] {
        let back: LifecycleEvent = serde_json::from_str(&serde_json::to_string(&event)?)?;
        assert_eq!(back, event);
    }
    Ok(())
}

#[test]
fn restore_snapshot_is_only_advertised_by_snapshot_restoring_providers() {
    for kind in [
        ProviderKind::LocalDocker,
        ProviderKind::K8sMicroVm,
        ProviderKind::Daytona,
        ProviderKind::Modal,
        ProviderKind::E2b,
        ProviderKind::Vercel,
        ProviderKind::Space,
        ProviderKind::Subprocess,
        ProviderKind::KubeVirt,
        ProviderKind::AzureContainerApps,
        ProviderKind::V8Wasm,
        ProviderKind::Proxmox,
        ProviderKind::Fly,
        ProviderKind::Mock,
    ] {
        assert!(
            !phases_supported_by(kind).contains(&SandboxPhase::RestoreSnapshot),
            "{kind:?} must not advertise restore_snapshot"
        );
    }
    for kind in [
        ProviderKind::Hyperlight,
        ProviderKind::Substrate,
        ProviderKind::Firecracker,
        ProviderKind::Hluk,
    ] {
        assert!(
            phases_supported_by(kind).contains(&SandboxPhase::RestoreSnapshot),
            "{kind:?} must advertise restore_snapshot"
        );
    }
    assert!(phases_supported_by(ProviderKind::Hyperlight).contains(&SandboxPhase::Snapshotting));
    assert_eq!(phases_supported_by(ProviderKind::Mock), &[SandboxPhase::Allocate]);
    assert_eq!(phases_supported_by(ProviderKind::V8Wasm).len(), 4);
}

#[test]
fn every_phase_has_a_label() {
    assert_eq!(sandbox_phase_label(SandboxPhase::Allocate), "Allocate sandbox");
    assert_eq!(sandbox_phase_label(SandboxPhase::RestoreSnapshot), "Restore snapshot");
    assert_eq!(sandbox_phase_label(SandboxPhase::MountFilesystem), "Mount filesystem");
    assert_eq!(sandbox_phase_label(SandboxPhase::PrepareWorkspace), "Prepare workspace");
    assert_eq!(sandbox_phase_label(SandboxPhase::PullImage), "Pull image");
    assert_eq!(sandbox_phase_label(SandboxPhase::PullingImage), "Pulling image");
    assert_eq!(sandbox_phase_label(SandboxPhase::BootRuntime), "Boot runtime");
    assert_eq!(sandbox_phase_label(SandboxPhase::Execute), "Execute");
    assert_eq!(sandbox_phase_label(SandboxPhase::Snapshotting), "Snapshotting");
    assert_eq!(sandbox_phase_label(SandboxPhase::Reclaim), "Reclaim sandbox");
}

#[test]
fn the_timer_reports_elapsed_time_and_detail() -> TestResult {
    let timer = PhaseTimer::start(SandboxPhase::Execute);
    assert_eq!(
        timer.phase_started(),
        LifecycleEvent::PhaseStarted {
            phase: SandboxPhase::Execute,
            detail: None
        }
    );
    std::thread::sleep(Duration::from_millis(2));
    let LifecycleEvent::PhaseCompleted {
        duration_ms, detail, ..
    } = timer.complete()
    else {
        return Err("expected a completed event".into());
    };
    assert!(duration_ms >= 1);
    assert_eq!(detail, None);

    let failed = PhaseTimer::start_with_detail(SandboxPhase::PullImage, Some("img".into())).fail("boom");
    assert!(matches!(
        failed,
        LifecycleEvent::PhaseFailed { phase: SandboxPhase::PullImage, ref error, detail: Some(ref detail), .. }
            if error == "boom" && detail == "img"
    ));
    Ok(())
}

#[tokio::test]
async fn a_successful_phase_emits_started_then_completed() -> TestResult {
    let recorder = Recorder::default();
    let value = run_phase(Some(&recorder), SandboxPhase::Allocate, Some("d".into()), || async {
        Ok::<_, std::io::Error>(7)
    })
    .await?;
    assert_eq!(value, 7);
    let events = recorder.events()?;
    assert!(matches!(
        events.as_slice(),
        [
            LifecycleEvent::PhaseStarted { .. },
            LifecycleEvent::PhaseCompleted { detail: Some(_), .. }
        ]
    ));
    Ok(())
}

#[tokio::test]
async fn a_failed_phase_emits_started_then_failed_and_returns_the_error() -> TestResult {
    let recorder = Recorder::default();
    let result = run_phase(Some(&recorder), SandboxPhase::BootRuntime, None, || async {
        Err::<(), _>(std::io::Error::other("no boot"))
    })
    .await;
    assert_eq!(result.err().map(|error| error.to_string()), Some("no boot".to_string()));
    let events = recorder.events()?;
    assert!(matches!(
        events.as_slice(),
        [
            LifecycleEvent::PhaseStarted { .. },
            LifecycleEvent::PhaseFailed { error, .. }
        ] if error == "no boot"
    ));
    Ok(())
}

#[tokio::test]
async fn a_phase_without_an_emitter_just_runs() -> TestResult {
    let none: Option<&NoopLifecycleEmitter> = None;
    assert_eq!(
        run_phase(none, SandboxPhase::Execute, None, || async {
            Ok::<_, std::io::Error>(1)
        })
        .await?,
        1
    );
    assert!(
        run_phase(none, SandboxPhase::Execute, None, || async {
            Err::<(), _>(std::io::Error::other("x"))
        })
        .await
        .is_err()
    );
    let noop = NoopLifecycleEmitter;
    noop.emit(LifecycleEvent::Output {
        stream: OutputStream::Stderr,
        text: String::new(),
    })
    .await;
    assert_eq!(
        run_phase(Some(&noop), SandboxPhase::Execute, None, || async {
            Ok::<_, std::io::Error>(2)
        })
        .await?,
        2
    );
    Ok(())
}

#[tokio::test]
async fn the_emitter_can_be_a_trait_object() -> TestResult {
    let recorder = Recorder::default();
    let dynamic: &dyn LifecycleEmitter = &recorder;
    run_phase(Some(dynamic), SandboxPhase::Reclaim, None, || async {
        Ok::<_, std::io::Error>(())
    })
    .await?;
    assert_eq!(recorder.events()?.len(), 2);
    Ok(())
}

#[test]
fn the_lifecycle_subject_names_the_lease() {
    let lease = LeaseId::new("lease-1");
    assert_eq!(lifecycle_subject(&lease), "sandbox.evidence.lease-1.lifecycle");
}
