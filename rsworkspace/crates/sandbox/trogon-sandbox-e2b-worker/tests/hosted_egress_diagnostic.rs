//! Diagnostic observations of how a real E2B sandbox behaves under each deny-all request shape. They stay separate
//! from conformance acceptance. Native create responses and authentication material stay in memory.
//!
//! Skipped unless `SANDBOX_E2B_EGRESS_DIAGNOSTIC_TEST=1`. It reads `E2B_API_KEY`, optionally `E2B_API_URL` and
//! `E2B_DOMAIN`, and `SANDBOX_E2B_TEMPLATE` (the template the sandboxes are created from).

mod hosted_support;

use std::collections::BTreeMap;
use std::panic::AssertUnwindSafe;
use std::time::Duration;

use futures::FutureExt;
use hosted_support::{Boxed, TestResult, e2b_api, e2b_config, require_gate, var};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use trogon_sandbox_contract::{RunCommandRequest, RunCommandResponse};
use trogon_sandbox_e2b_worker::{
    ApiError, E2bApi, E2bConfig, E2bExec, Envd, EnvdTarget, InternetAccess, LeaseSandbox, NetworkRules, NewSandbox,
};

const OWNER: &str = "sandbox-egress-diagnostic";
const WORK_LIMIT: Duration = Duration::from_secs(150);
const PROBE: &str = r#"
import errno, hashlib, http.client, json, socket, ssl
ENDPOINTS = [
    ("original", "1.1.1.1", 443, "1.1.1.1", "/cdn-cgi/trace"),
    ("pinned_httpbin", "44.209.237.184", 443, "httpbin.org", "/get"),
]
LIMIT = 65536
def error_kind(error):
    if isinstance(error, ssl.SSLCertVerificationError): return "tls_certificate"
    if isinstance(error, ssl.SSLError): return "tls_protocol"
    if isinstance(error, (socket.timeout, TimeoutError)): return "timeout"
    if isinstance(error, http.client.HTTPException): return "invalid_http"
    number = getattr(error, "errno", None)
    if number == errno.ECONNREFUSED: return "refused"
    if number in (errno.ECONNRESET, errno.EPIPE): return "reset"
    if number in (errno.ENETUNREACH, errno.EHOSTUNREACH): return "unreachable"
    if number in (errno.EACCES, errno.EPERM): return "permission"
    if isinstance(error, OSError): return "io"
    return "unexpected"
def connect(ip, port):
    connection = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    connection.settimeout(3)
    try:
        connection.connect((ip, port))
        return connection
    except BaseException:
        connection.close()
        raise
def probe(endpoint):
    name, ip, port, server_name, path = endpoint
    result = {"name": name}
    try:
        connection = connect(ip, port)
        connection.close()
        result["raw"] = {"outcome": "connected"}
    except Exception as error:
        result["raw"] = {"outcome": "failed", "error": error_kind(error),
                         "errno": getattr(error, "errno", None)}
    stage, certificate, status = "connect", None, None
    connection = None
    try:
        context = ssl.create_default_context()
        if not context.check_hostname or context.verify_mode != ssl.CERT_REQUIRED:
            raise RuntimeError()
        connection = connect(ip, port)
        stage = "tls"
        connection = context.wrap_socket(connection, server_hostname=server_name)
        certificate = hashlib.sha256(connection.getpeercert(binary_form=True)).hexdigest()
        stage = "http"
        request = ("GET " + path + " HTTP/1.1\r\nHost: " + server_name +
                   "\r\nConnection: close\r\nUser-Agent: sandbox-egress-diagnostic\r\n\r\n")
        connection.sendall(request.encode("ascii"))
        response = http.client.HTTPResponse(connection)
        response.begin()
        status = response.status
        body = response.read(LIMIT + 1)
        if len(body) > LIMIT:
            error = "oversized"
        elif status != 200:
            error = "http_rejected"
        elif name == "original":
            lines = set(body.splitlines())
            error = None if b"h=1.1.1.1" in lines and b"visit_scheme=https" in lines else "origin_unrecognized"
        else:
            document = json.loads(body)
            error = None if isinstance(document, dict) and document.get("url") == "https://httpbin.org/get" else "origin_unrecognized"
        if error is None:
            result["delivery"] = {
                "outcome": "expected_response", "status": status,
                "certificate_sha256": certificate,
                "body_sha256": hashlib.sha256(body).hexdigest(),
            }
        else:
            result["delivery"] = {
                "outcome": "failed", "stage": stage, "error": error,
                "status": status, "errno": None, "certificate_sha256": certificate,
            }
    except Exception as error:
        result["delivery"] = {
            "outcome": "failed", "stage": stage, "error": error_kind(error),
            "status": status, "errno": getattr(error, "errno", None),
            "certificate_sha256": certificate,
        }
    finally:
        if connection is not None:
            connection.close()
    return result
print(json.dumps({"endpoints": [probe(endpoint) for endpoint in ENDPOINTS]}))
"#;

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum Shape {
    AllowAll,
    ShortformDeny,
    ExplicitDeny,
}

impl Shape {
    fn name(self) -> &'static str {
        match self {
            Self::AllowAll => "allow_all",
            Self::ShortformDeny => "shortform_deny",
            Self::ExplicitDeny => "explicit_deny",
        }
    }
}

/// A sandbox the lane created and therefore must delete.
struct Owned {
    shape: Shape,
    id: String,
    raw: trogon_sandbox_e2b_worker::SandboxInfo,
    running: bool,
}

async fn create(api: &E2bApi, shape: Shape, template: &str, nonce: &str, owned: &mut Vec<Owned>) -> TestResult {
    let mut request = NewSandbox {
        template_id: template.to_owned(),
        timeout: 300,
        metadata: BTreeMap::from([(OWNER.to_owned(), format!("{nonce}-{}", shape.name()))]),
        ..Default::default()
    };
    match shape {
        Shape::AllowAll => {}
        Shape::ShortformDeny => request.allow_internet_access = Some(InternetAccess::Denied),
        Shape::ExplicitDeny => {
            request.allow_internet_access = Some(InternetAccess::Denied);
            request.network = Some(NetworkRules {
                allow_out: Vec::new(),
                deny_out: vec!["0.0.0.0/0".to_owned()],
            });
        }
    }
    let raw = api.create_sandbox(&request).await?;
    let id = raw.sandbox_id.clone();
    owned.push(Owned {
        shape,
        id,
        raw,
        running: false,
    });
    Ok(())
}

async fn inspect(api: &E2bApi, owned: &mut Owned) -> TestResult {
    let detail = api.get_sandbox(&owned.id).await?;
    if detail.state != "running" {
        return Err("an owned native sandbox is not running".into());
    }
    owned.running = true;
    Ok(())
}

/// Deletes the sandbox and waits for the exact id to read as gone.
async fn cleanup(api: &E2bApi, owned: &Owned) -> Value {
    let deleted = api.delete_sandbox(&owned.id).await.is_ok();
    let until = tokio::time::Instant::now() + Duration::from_secs(20);
    let mut absent = false;
    loop {
        match api.get_sandbox(&owned.id).await {
            Err(ApiError::NotFound(_)) => {
                absent = true;
                break;
            }
            Ok(_) if tokio::time::Instant::now() < until => tokio::time::sleep(Duration::from_millis(250)).await,
            _ => break,
        }
    }
    json!({"shape": owned.shape, "sandbox_id": owned.running.then_some(&owned.id), "delete_accepted": deleted, "exact_absent": absent})
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
enum RawOutcome {
    Connected,
    Failed { error: String, errno: Option<i32> },
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
enum Delivery {
    ExpectedResponse {
        status: u16,
        certificate_sha256: String,
        body_sha256: String,
    },
    Failed {
        stage: String,
        error: String,
        status: Option<u16>,
        errno: Option<i32>,
        certificate_sha256: Option<String>,
    },
}

#[derive(Debug, Deserialize, Serialize)]
struct Endpoint {
    name: String,
    raw: RawOutcome,
    delivery: Delivery,
}

#[derive(Debug, Deserialize, Serialize)]
struct ProbeBatch {
    endpoints: Vec<Endpoint>,
}

impl ProbeBatch {
    fn certificate(&self, index: usize) -> Option<&str> {
        match &self.endpoints[index].delivery {
            Delivery::ExpectedResponse { certificate_sha256, .. } => Some(certificate_sha256),
            Delivery::Failed { certificate_sha256, .. } => certificate_sha256.as_deref(),
        }
    }

    fn healthy(&self) -> bool {
        self.endpoints.iter().all(|endpoint| {
            matches!(endpoint.raw, RawOutcome::Connected)
                && matches!(endpoint.delivery, Delivery::ExpectedResponse { status: 200, .. })
        })
    }
}

async fn probe(config: &E2bConfig, exec: &E2bExec, owned: &Owned) -> Result<ProbeBatch, Boxed> {
    let info = &owned.raw;
    if info.envd_access_token.as_ref().is_none_or(String::is_empty) {
        return Err("native create did not supply exact secured controller access".into());
    }
    let sandbox = LeaseSandbox {
        sandbox_id: owned.id.clone(),
        target: EnvdTarget::for_sandbox(
            config,
            &owned.id,
            info.domain.as_deref(),
            info.envd_access_token.clone(),
        )?,
        traffic_token: info.traffic_access_token.clone(),
        domain: info.domain.clone().unwrap_or_else(|| config.domain.clone()),
        exposed_ports: vec![],
    };
    let request: RunCommandRequest = serde_json::from_value(json!({
        "lease_id": format!("egress-diagnostic-{}", owned.shape.name()),
        "command": ["python3", "-c", PROBE],
        "cwd": "/tmp",
        "timeout_seconds": 45,
        "max_output_bytes": 32768,
    }))?;
    let RunCommandResponse::Completed(output) = exec.handle_run_command(&sandbox, &request).await else {
        return Err("native diagnostic command did not settle".into());
    };
    if output.exit_code != 0 || !output.timed_out.is_within_deadline() || !output.truncated.is_complete() {
        return Err("native diagnostic tool or command is unavailable".into());
    }
    let batch: ProbeBatch = serde_json::from_slice(&output.stdout)?;
    let names: Vec<&str> = batch.endpoints.iter().map(|endpoint| endpoint.name.as_str()).collect();
    if names != ["original", "pinned_httpbin"] {
        return Err("native diagnostic endpoint identities changed".into());
    }
    for index in 0..batch.endpoints.len() {
        let digest_is_valid = |digest: &str| digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit());
        if batch.certificate(index).is_some_and(|digest| !digest_is_valid(digest)) {
            return Err("native diagnostic certificate digest is invalid".into());
        }
        if let Delivery::ExpectedResponse { body_sha256, .. } = &batch.endpoints[index].delivery
            && !digest_is_valid(body_sha256)
        {
            return Err("native diagnostic digest is invalid".into());
        }
    }
    Ok(batch)
}

#[tokio::test]
#[ignore = "paid direct E2B diagnostic: set SANDBOX_E2B_EGRESS_DIAGNOSTIC_TEST=1 with E2B_API_KEY and SANDBOX_E2B_TEMPLATE, then use --ignored"]
async fn direct_shortform_and_explicit_egress_observations_preserve_the_raw_failure() -> TestResult {
    require_gate("SANDBOX_E2B_EGRESS_DIAGNOSTIC_TEST")?;
    let template = var("SANDBOX_E2B_TEMPLATE")?;
    let api = e2b_api()?;
    let config = e2b_config()?;
    let exec = E2bExec::new(Envd::new(Duration::from_secs(60))?);
    let nonce = uuid::Uuid::new_v4().to_string();
    let mut owned = Vec::new();
    let mut observations: Vec<(&str, Option<ProbeBatch>)> = Vec::new();
    let work = AssertUnwindSafe(tokio::time::timeout(WORK_LIMIT, async {
        for shape in [Shape::AllowAll, Shape::ShortformDeny, Shape::ExplicitDeny] {
            create(&api, shape, &template, &nonce, &mut owned).await?;
        }
        for sandbox in &mut owned {
            inspect(&api, sandbox).await?;
        }
        for (phase, index) in [
            ("control_before", 0),
            ("shortform_deny", 1),
            ("explicit_deny", 2),
            ("control_after", 0),
        ] {
            observations.push((phase, probe(&config, &exec, &owned[index]).await.ok()));
        }
        Ok::<(), Boxed>(())
    }))
    .catch_unwind()
    .await;
    let mut receipts = Vec::new();
    for sandbox in &owned {
        receipts.push(cleanup(&api, sandbox).await);
    }

    let healthy = |index: usize| {
        observations
            .get(index)
            .and_then(|(_, batch)| batch.as_ref())
            .filter(|batch| batch.healthy())
    };
    let controls_healthy = observations.len() == 4 && healthy(0).is_some() && healthy(3).is_some();
    let controls_match = controls_healthy
        && healthy(0)
            .zip(healthy(3))
            .is_some_and(|(before, after)| (0..2).all(|index| before.certificate(index) == after.certificate(index)));
    let denied: Vec<&ProbeBatch> = observations
        .iter()
        .filter(|(phase, _)| matches!(*phase, "shortform_deny" | "explicit_deny"))
        .filter_map(|(_, batch)| batch.as_ref())
        .collect();
    let raw_connected = denied
        .iter()
        .any(|batch| matches!(batch.endpoints[0].raw, RawOutcome::Connected));
    let raw_failure_retained = if raw_connected {
        Some(true)
    } else if denied.len() == 2 {
        Some(false)
    } else {
        None
    };
    let (mut tls_delivery, mut certificate_unknown) = (false, false);
    if let Some(control) = healthy(0).filter(|_| controls_match) {
        for (_, batch) in &observations[1..3] {
            let Some(batch) = batch else { continue };
            for index in 0..batch.endpoints.len() {
                let authenticated = batch.certificate(index).is_some();
                let matches_control = authenticated && batch.certificate(index) == control.certificate(index);
                tls_delivery |= matches_control;
                certificate_unknown |= authenticated && !matches_control;
            }
        }
    }
    let work_status = match &work {
        Ok(Ok(Ok(()))) => "completed",
        Ok(Ok(Err(_))) => "error",
        Ok(Err(_)) => "timeout",
        Err(_) => "panic",
    };
    eprintln!(
        "{}",
        json!({
            "diagnostic_only": true,
            "conformance_acceptance": false,
            "work_status": work_status,
            "controls_healthy": controls_healthy,
            "control_certificates_stable": controls_match,
            "original_raw_failure_retained": raw_failure_retained,
            "observations": observations,
            "cleanup": receipts,
        })
    );
    let cleaned = owned.len() == 3
        && receipts
            .iter()
            .all(|receipt| receipt["delete_accepted"] == true && receipt["exact_absent"] == true);
    if !cleaned {
        return Err("direct diagnostic exact planned cleanup is incomplete".into());
    }
    if work_status != "completed" {
        return Err("direct diagnostic setup or command outcome is unknown".into());
    }
    if !controls_match {
        return Err("before/after endpoint health or origin identity is setup-unknown".into());
    }
    if observations.iter().any(|(_, batch)| batch.is_none()) {
        return Err("a guest diagnostic outcome is unknown".into());
    }
    if certificate_unknown {
        return Err("deny-all authenticated a different TLS peer; delivery is unknown".into());
    }
    if tls_delivery {
        return Err("deny-all completed an authenticated TLS handshake matching the healthy origin".into());
    }
    if raw_failure_retained != Some(false) {
        return Err("deny-all retained raw TCP success at 1.1.1.1:443; application observations are separate".into());
    }
    Ok(())
}
