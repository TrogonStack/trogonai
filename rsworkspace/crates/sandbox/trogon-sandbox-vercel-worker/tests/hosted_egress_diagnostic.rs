//! A diagnostic of what the public provider does under an allow-all and a deny-all policy. Its observations do not
//! replace the conformance suite's verdict, and the paid lane only reports them.
//!
//! Skipped unless `SANDBOX_VERCEL_HOSTED_TEST=1`; see `hosted_support` for the other variables.

mod hosted_egress_support;
mod hosted_support;

use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::time::Duration;

use futures::FutureExt;
use hosted_egress_support::{Errno, Observation, Stdout, classify, errno_class};
use hosted_support::{Boxed, GATE, TestResult, api, confirm_absent, exact_id, require, verify_running};
use serde::Serialize;
use serde_json::{Value, json};
use trogon_sandbox_contract::{LeaseId, LeaseRequest, RunCommandRequest};
use trogon_sandbox_protocol::Provider;
use trogon_sandbox_vercel_worker::{LeaseSandbox, VercelApi, VercelExec, VercelPlan, VercelWorkerProvider};
use uuid::Uuid;

// The explicit node24 runtime is documented as a legacy image. Neither the `nc` implementation nor the presence of
// Python is a contract, so the diagnostic asks the guest which probe it can run before it probes anything.
const PREFLIGHT: &str = "if command -v nc >/dev/null 2>&1; then printf 'NC\\n'; \
        elif command -v python3 >/dev/null 2>&1; then \
        python3 -c 'import socket,sys; assert sys.version_info.major == 3; print(\"PYTHON3\")'; \
        else printf 'NO_PROBE\\n'; fi";
const NC_CONNECT: &str = "nc -z -w 3 \"$1\" \"$2\" >/dev/null 2>&1; status=$?; \
                case \"$status\" in 0) printf 'CONNECTED\\n';; 1) printf 'DENIED\\n';; *) exit 1;; esac";
const PYTHON_CONNECT: &str = r"import errno,socket,sys
connection=socket.socket(socket.AF_INET6 if ':' in sys.argv[1] else socket.AF_INET,socket.SOCK_STREAM)
connection.settimeout(3)
try:
 try:
  connection.connect((sys.argv[1],int(sys.argv[2])))
 except TimeoutError:
  print('DENIED')
 except OSError as error:
  if error.errno not in (errno.EACCES,errno.EPERM,errno.ENETUNREACH,errno.EHOSTUNREACH,errno.ETIMEDOUT,errno.ECONNREFUSED):
   raise
  print('DENIED')
 else:
  print('CONNECTED')
finally:
 connection.close()
";

const BODY_BOUND: Duration = Duration::from_secs(240);
const CLEANUP_BOUND: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Role {
    Control,
    Restricted,
}

impl Role {
    fn label(self) -> &'static str {
        match self {
            Self::Control => "control",
            Self::Restricted => "restricted",
        }
    }

    fn policy(self) -> &'static str {
        match self {
            Self::Control => "allow_all",
            Self::Restricted => "deny_all",
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum Interpretation {
    SetupFailure,
    IncompleteCommand,
    DeniedWithHealthyControls,
    RestrictedConnectedWithHealthyControls,
    Inconclusive,
}

#[derive(Serialize)]
struct Allocation {
    role: Role,
    lease_id: String,
    native_name: Option<String>,
    session_id: Option<String>,
    native_verified: bool,
    cleanup_native_verified: bool,
    product_cancel_accepted: bool,
    exact_absent: bool,
}

#[derive(Serialize)]
struct Receipt {
    evidence: &'static str,
    allocations: Vec<Allocation>,
    observations: Vec<Observation>,
    control_before_healthy: bool,
    control_after_healthy: bool,
    interpretation: Interpretation,
    product_cleanup: bool,
}

/// One sandbox the diagnostic leased and so must remove.
struct Held {
    role: Role,
    lease: LeaseId,
    name: String,
    sandbox: Option<LeaseSandbox>,
    index: usize,
}

fn lease_request(role: Role, key: &str) -> Result<LeaseRequest, Boxed> {
    Ok(serde_json::from_value(json!({
        "tenant": "vercel-egress-diagnostic",
        "provider_pref": ["vercel"],
        "spec": {
            "image": {"kind": "registry", "reference": {"registry": "", "repository": "node24"}},
            "workspace": {"empty": {}},
            "cpu_seconds": 10,
            "memory_mib": 512,
            "env": {},
            "network_policy": role.policy(),
        },
        "ttl_seconds": 600,
        "idempotency_key": key,
        "persistent": true,
    }))?)
}

async fn run_phase(
    provider: &VercelWorkerProvider,
    held: &Held,
    phase: &'static str,
    argv: &[&str],
) -> Result<Observation, Boxed> {
    let sandbox = held.sandbox.as_ref().ok_or("the exact held session is unavailable")?;
    let request: RunCommandRequest = serde_json::from_value(json!({
        "lease_id": held.lease.as_str(),
        "command": argv,
        "timeout_seconds": 20,
        "max_output_bytes": 4096,
    }))?;
    let response = provider.exec().handle_run_command(sandbox, &request).await;
    Ok(classify(phase, held.role.label(), &response))
}

fn original_connect(tool: Stdout) -> Vec<&'static str> {
    match tool {
        Stdout::Nc => vec!["sh", "-c", NC_CONNECT, "egress-probe", "1.1.1.1", "443"],
        Stdout::Python3 => vec!["python3", "-c", PYTHON_CONNECT, "1.1.1.1", "443"],
        _ => Vec::new(),
    }
}

async fn allocate(
    provider: &VercelWorkerProvider,
    verifier: &VercelApi,
    role: Role,
    held: &mut Vec<Held>,
    receipt: &mut Receipt,
) -> TestResult {
    let lease = LeaseId::new(format!("vercel-egress-diagnostic-{}", Uuid::new_v4()));
    let grant = provider
        .lease(&lease_request(role, lease.as_str())?, &lease)
        .await
        .map_err(|_| "the native allocation failed; an incomplete reply needs reconciliation")?;
    let name = grant.sandbox_id.as_str().to_owned();
    let index = receipt.allocations.len();
    receipt.allocations.push(Allocation {
        role,
        lease_id: lease.as_str().to_owned(),
        native_name: exact_id(&name).then(|| name.clone()),
        session_id: None,
        native_verified: false,
        cleanup_native_verified: false,
        product_cancel_accepted: false,
        exact_absent: false,
    });
    let sandbox = provider.sandbox_for_lease(&lease).await;
    receipt.allocations[index].session_id = sandbox
        .as_ref()
        .filter(|sandbox| exact_id(&sandbox.session_id))
        .map(|sandbox| sandbox.session_id.clone());
    held.push(Held {
        role,
        lease,
        name,
        sandbox,
        index,
    });
    let owned = held.last().ok_or("the allocation was not retained")?;
    let session = owned.sandbox.as_ref().map(|sandbox| sandbox.session_id.as_str());
    let verified = verify_running(verifier, &owned.name, session.unwrap_or_default()).await;
    receipt.allocations[index].native_verified = verified.is_ok();
    verified
}

fn interpret(before: &Observation, restricted: &Observation, after: &Observation, receipt: &mut Receipt) {
    let healthy = |observation: &Observation| observation.usable() && observation.output.stdout == Stdout::Connected;
    receipt.control_before_healthy = healthy(before);
    receipt.control_after_healthy = healthy(after);
    receipt.interpretation = if !receipt.control_before_healthy || !receipt.control_after_healthy {
        Interpretation::Inconclusive
    } else if !restricted.usable() {
        Interpretation::IncompleteCommand
    } else {
        match restricted.output.stdout {
            Stdout::Connected => Interpretation::RestrictedConnectedWithHealthyControls,
            Stdout::Denied => Interpretation::DeniedWithHealthyControls,
            _ => Interpretation::Inconclusive,
        }
    };
}

async fn observe(
    provider: &VercelWorkerProvider,
    verifier: &VercelApi,
    held: &mut Vec<Held>,
    receipt: &mut Receipt,
) -> TestResult {
    allocate(provider, verifier, Role::Control, held, receipt).await?;
    allocate(provider, verifier, Role::Restricted, held, receipt).await?;
    let (control, restricted) = (&held[0], &held[1]);
    let independent = control.name != restricted.name
        && control.sandbox.as_ref().map(|sandbox| &sandbox.session_id)
            != restricted.sandbox.as_ref().map(|sandbox| &sandbox.session_id);
    if !independent {
        return Err("the two network controls must be independent native allocations".into());
    }
    let preflight = run_phase(provider, control, "preflight_control", &["sh", "-c", PREFLIGHT]).await?;
    receipt.observations.push(preflight);
    let restricted_preflight =
        run_phase(provider, restricted, "preflight_restricted", &["sh", "-c", PREFLIGHT]).await?;
    receipt.observations.push(restricted_preflight);
    let argv = original_connect(preflight.output.stdout);
    if !preflight.usable() || argv.is_empty() {
        receipt.interpretation = Interpretation::SetupFailure;
        return Ok(());
    }
    let before = run_phase(provider, control, "control_before", &argv).await?;
    let denied = run_phase(provider, restricted, "restricted", &argv).await?;
    let after = run_phase(provider, control, "control_after", &argv).await?;
    receipt.observations.extend([before, denied, after]);
    interpret(&before, &denied, &after, receipt);
    Ok(())
}

/// Cancels each held lease through the provider and proves its native sandbox is gone, while the control sandbox
/// stays reachable until the restricted one is confirmed gone.
async fn clean_up(
    provider: &VercelWorkerProvider,
    verifier: &VercelApi,
    held: &mut [Held],
    receipt: &mut Receipt,
) -> bool {
    let mut cleanup_ok = true;
    for owned in held.iter_mut() {
        if owned.sandbox.is_none() {
            owned.sandbox = provider.sandbox_for_lease(&owned.lease).await;
        }
    }
    for owned in held.iter().rev() {
        let session = owned.sandbox.as_ref().map(|sandbox| sandbox.session_id.as_str());
        let verified = verify_running(verifier, &owned.name, session.unwrap_or_default())
            .await
            .is_ok();
        let cleaned = verified
            && matches!(
                tokio::time::timeout(CLEANUP_BOUND, provider.cancel(&owned.lease)).await,
                Ok(Ok(()))
            );
        let absent = confirm_absent(verifier, &owned.name).await.is_ok();
        let allocation = &mut receipt.allocations[owned.index];
        allocation.cleanup_native_verified = verified;
        allocation.product_cancel_accepted = cleaned;
        allocation.exact_absent = absent;
        cleanup_ok &= cleaned && absent;
    }
    cleanup_ok
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "paid diagnostic: set SANDBOX_VERCEL_HOSTED_TEST=1 with VERCEL_TOKEN, VERCEL_TEAM_ID and VERCEL_PROJECT_ID"]
async fn actual_public_provider_egress_reports_strict_original_probe_diagnostics() -> TestResult {
    require(GATE)?;
    let provider_api = api(Duration::from_secs(60))?;
    let verifier = api(Duration::from_secs(10))?;
    let provider = VercelWorkerProvider::new(
        provider_api.clone(),
        "node24",
        1,
        VercelPlan::default(),
        Arc::new(VercelExec::new(provider_api)),
    );
    let mut held = Vec::new();
    let mut receipt = Receipt {
        evidence: "direct_public_provider_diagnostic_not_conformance_acceptance",
        allocations: Vec::new(),
        observations: Vec::new(),
        control_before_healthy: false,
        control_after_healthy: false,
        interpretation: Interpretation::Inconclusive,
        product_cleanup: false,
    };
    let observed = AssertUnwindSafe(tokio::time::timeout(
        BODY_BOUND,
        observe(&provider, &verifier, &mut held, &mut receipt),
    ))
    .catch_unwind()
    .await;
    let cleanup_ok = clean_up(&provider, &verifier, &mut held, &mut receipt).await;
    receipt.product_cleanup = cleanup_ok && held.len() == 2;
    eprintln!(
        "Vercel original egress diagnostic receipt: {}",
        serde_json::to_string(&receipt)?
    );
    if !cleanup_ok {
        return Err("exact owned cleanup failed; retained allocations need reconciliation".into());
    }
    match observed {
        Ok(Ok(result)) => result,
        Ok(Err(_)) => Err("the diagnostic body exceeded its bound".into()),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

#[test]
fn socket_errno_classification_serializes_only_closed_labels() -> TestResult {
    let reset = errno_class(b"ConnectionResetError: [Errno 104] opaque fixture message");
    let canceled = errno_class(b"OSError: [Errno 125] another opaque fixture message");
    assert_eq!((reset, canceled), (Errno::Econnreset, Errno::Ecanceled));
    assert_eq!(serde_json::to_value(reset)?, Value::from("econnreset"));
    assert_eq!(serde_json::to_value(canceled)?, Value::from("ecanceled"));
    Ok(())
}

#[test]
fn malformed_or_ambiguous_errno_cannot_invent_a_known_socket_error() {
    assert_eq!(errno_class(b"generic reset text"), Errno::NotReported);
    assert_eq!(errno_class(b"[Errno 999] unknown"), Errno::Other);
    assert_eq!(errno_class(b"[Errno 104] one [Errno 125] two"), Errno::Ambiguous);
    for value in [b"[Errno 104".as_slice(), b"[Errno nope]", b"[Errno 9999999999]"] {
        assert_eq!(errno_class(value), Errno::Malformed);
    }
}
