use crate::k8s_ops::{
    ClaimedWarmPod, FencedPatchOutcome, K8sLaunchOptions, LeaseJobSweep, LeaseOwnerKind, PodExecOutput,
    RecoveredLeaseJob, RecoveredWarmLease,
};

#[test]
fn a_timed_out_command_reports_the_deadline_it_missed() {
    let output = PodExecOutput::timed_out_after(30);

    assert_eq!(output.exit_code, -1);
    assert!(output.timed_out);
    assert_eq!(output.stderr, b"command timed out after 30s");
    assert!(output.stdout.is_empty());
    assert!(!output.truncated.is_truncated());
    assert_eq!(output.clone(), output);
    assert!(format!("{output:?}").contains("timed_out"));
}

#[test]
fn recovered_leases_compare_and_clone_by_value() {
    let job = RecoveredLeaseJob {
        lease_id: "l".to_string(),
        tenant_id: None,
        job_name: "j".to_string(),
        pod_name: "p".to_string(),
    };
    let warm = RecoveredWarmLease {
        lease_id: "l".to_string(),
        pod_name: "p".to_string(),
        restore_secret_name: Some("r".to_string()),
    };
    let claimed = ClaimedWarmPod {
        name: "p".to_string(),
        restore_secret_name: None,
    };
    let sweep = LeaseJobSweep {
        live: vec![job.clone()],
        swept: vec!["gone".to_string()],
    };

    assert_eq!(job.clone(), job);
    assert_eq!(warm.clone(), warm);
    assert_eq!(claimed.clone(), claimed);
    assert_eq!(sweep.clone(), sweep);
    assert_ne!(sweep, LeaseJobSweep::default());
    assert!(format!("{job:?}{warm:?}{claimed:?}{sweep:?}").contains("gone"));
}

#[test]
fn launch_options_default_to_nothing_and_clone_by_value() {
    let options = K8sLaunchOptions {
        keep_alive: true,
        ..Default::default()
    };

    assert_eq!(options.clone(), options);
    assert_ne!(options, K8sLaunchOptions::default());
    assert!(format!("{options:?}").contains("keep_alive"));
}

#[test]
fn small_enums_copy_and_compare() {
    let kind = LeaseOwnerKind::Job;
    let outcome = FencedPatchOutcome::Applied;

    assert_eq!(kind, kind.clone());
    assert_ne!(kind, LeaseOwnerKind::Pod);
    assert_eq!(outcome, outcome.clone());
    assert_ne!(outcome, FencedPatchOutcome::PreconditionFailed);
    assert!(format!("{kind:?}{outcome:?}").contains("Applied"));
}
