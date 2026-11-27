use super::*;

/// A child process the lane owns, with bounded and redacted diagnostic tails of its output.
pub(super) struct ManagedChild {
    pub(super) role: &'static str,
    pub(super) child: Child,
    pub(super) pid: u32,
    pub(super) stdout: Arc<Mutex<String>>,
    pub(super) stderr: Arc<Mutex<String>>,
    pub(super) drainers: Vec<tokio::task::JoinHandle<()>>,
}

impl ManagedChild {
    pub(super) fn start(role: &'static str, command: &mut Command, secrets: &[String]) -> anyhow::Result<Self> {
        let mut child = command
            .kill_on_drop(true)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let pid = child.id().context("owned child omitted PID")?;
        let stdout = Arc::new(Mutex::new(String::new()));
        let stderr = Arc::new(Mutex::new(String::new()));
        let drainers = vec![
            tokio::spawn(drain_tail(
                child.stdout.take().context("owned stdout pipe missing")?,
                stdout.clone(),
                secrets.to_vec(),
            )),
            tokio::spawn(drain_tail(
                child.stderr.take().context("owned stderr pipe missing")?,
                stderr.clone(),
                secrets.to_vec(),
            )),
        ];
        Ok(Self {
            role,
            child,
            pid,
            stdout,
            stderr,
            drainers,
        })
    }

    pub(super) fn diagnostics(&self) -> Value {
        json!({"role":self.role, "pid":self.pid,
            "stdout_tail":*locked(&self.stdout), "stderr_tail":*locked(&self.stderr)})
    }

    pub(super) async fn stop(&mut self) -> anyhow::Result<()> {
        let mut forced = false;
        if self.child.try_wait()?.is_none() {
            let signal = output(Command::new("kill").args(["-INT", &self.pid.to_string()]), None).await?;
            ensure!(signal.status.success(), "could not stop the exact owned child");
            match tokio::time::timeout(Duration::from_secs(5), self.child.wait()).await {
                Ok(result) => {
                    result?;
                }
                Err(_) => {
                    forced = true;
                    self.child.start_kill()?;
                    tokio::time::timeout(Duration::from_secs(5), self.child.wait()).await??;
                }
            }
        }
        for drainer in &mut self.drainers {
            match tokio::time::timeout(Duration::from_secs(1), &mut *drainer).await {
                Ok(Ok(())) => {}
                _ => {
                    drainer.abort();
                    anyhow::bail!("owned diagnostic pipe did not settle");
                }
            }
        }
        ensure!(!forced, "owned child required forced-stop fallback");
        Ok(())
    }
}

/// Reads `input` to its end, keeping at most [`CAPTURE_BYTES`] and reporting whether more arrived.
async fn capture_bounded(mut input: impl AsyncRead + Unpin) -> anyhow::Result<(Vec<u8>, bool)> {
    let mut bytes = Vec::new();
    let mut overflow = false;
    let mut buffer = [0_u8; 4096];
    loop {
        let count = input.read(&mut buffer).await?;
        if count == 0 {
            return Ok((bytes, overflow));
        }
        if bytes.len().saturating_add(count) <= CAPTURE_BYTES && !overflow {
            bytes.extend_from_slice(&buffer[..count]);
        } else {
            overflow = true;
        }
    }
}

pub(super) async fn output(command: &mut Command, input: Option<&[u8]>) -> anyhow::Result<std::process::Output> {
    let mut child = command
        .kill_on_drop(true)
        .stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let stdout = tokio::spawn(capture_bounded(child.stdout.take().context("owned stdout missing")?));
    let stderr = tokio::spawn(capture_bounded(child.stderr.take().context("owned stderr missing")?));
    if let Some(input) = input {
        let mut stdin = child.stdin.take().context("owned command stdin missing")?;
        if !matches!(tokio::time::timeout(WAIT, stdin.write_all(input)).await, Ok(Ok(()))) {
            let _ = child.start_kill();
            let _ = tokio::time::timeout(WAIT, child.wait()).await;
            stdout.abort();
            stderr.abort();
            anyhow::bail!("owned command input did not settle");
        }
    }
    let status = match tokio::time::timeout(Duration::from_secs(240), child.wait()).await {
        Ok(status) => status?,
        Err(_) => {
            child.start_kill()?;
            tokio::time::timeout(WAIT, child.wait()).await??;
            stdout.abort();
            stderr.abort();
            anyhow::bail!("owned command exceeded its execution bound and was reaped");
        }
    };
    let (stdout_bytes, stdout_overflow) = tokio::time::timeout(WAIT, stdout).await???;
    let (stderr_bytes, stderr_overflow) = tokio::time::timeout(WAIT, stderr).await???;
    ensure!(
        !stdout_overflow && !stderr_overflow,
        "owned command output exceeded its capture bound"
    );
    Ok(std::process::Output {
        status,
        stdout: stdout_bytes,
        stderr: stderr_bytes,
    })
}

pub(super) async fn successful(command: &mut Command, input: Option<&[u8]>) -> anyhow::Result<Vec<u8>> {
    let secrets = command
        .as_std()
        .get_envs()
        .filter_map(|(_, value)| value)
        .map(|value| value.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    let result = output(command, input).await?;
    ensure!(
        result.status.success(),
        "owned fixture command {} failed: status={:?}; stdout={}; stderr={}",
        command.as_std().get_program().to_string_lossy(),
        result.status.code(),
        redact(&result.stdout, &secrets),
        redact(&result.stderr, &secrets)
    );
    Ok(result.stdout)
}

pub(super) async fn grpc(address: &str, token: &str, method: &str, value: Value) -> anyhow::Result<Value> {
    let body = serde_json::to_vec(&value)?;
    let data = successful(
        Command::new("grpcurl").env("SPICEDB_TOKEN", token).args([
            "-plaintext",
            "-expand-headers",
            "-H",
            "Authorization: Bearer ${SPICEDB_TOKEN}",
            "-d",
            "@",
            address,
            method,
        ]),
        Some(&body),
    )
    .await?;
    Ok(serde_json::from_slice(&data)?)
}

pub(super) async fn nats(children: &mut Vec<ManagedChild>, directory: &Path) -> anyhow::Result<(Client, String)> {
    let mut command = Command::new("nats-server");
    command
        .args(["-a", "127.0.0.1", "-p", "-1", "-js", "-sd"])
        .arg(directory)
        .arg("--ports_file_dir")
        .arg(directory);
    let child = ManagedChild::start("nats", &mut command, &[])?;
    let pid = child.pid;
    children.push(child);
    let ports = directory.join(format!("nats-server_{pid}.ports"));
    tokio::time::timeout(WAIT, async {
        loop {
            if let Ok(bytes) = std::fs::read(&ports)
                && let Ok(value) = serde_json::from_slice::<Value>(&bytes)
                && let Some(url) = value["nats"][0].as_str()
                && let Ok(client) = async_nats::connect(url).await
            {
                return Ok((client, url.to_owned()));
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .context("owned NATS did not become ready")?
}

/// Sends a raw lease request, optionally carrying `bearer` as its API key, without the client SDK.
pub(super) async fn request(
    client: &Client,
    request: &LeaseRequest,
    bearer: Option<&str>,
) -> anyhow::Result<LeaseResponse> {
    let mut headers = HeaderMap::new();
    if let Some(key) = bearer {
        headers.insert("Authorization", format!("Bearer {key}"));
    }
    let reply = client
        .send_request(
            LEASE_REQUEST,
            async_nats::Request::new()
                .headers(headers)
                .payload(serde_json::to_vec(request)?.into())
                .timeout(Some(WAIT)),
        )
        .await?;
    Ok(serde_json::from_slice(&reply.payload)?)
}

pub(super) fn denial_summary(response: &LeaseResponse, secrets: &[String]) -> Value {
    match response {
        LeaseResponse::Denied(error) => {
            let encoded = serde_json::to_value(error).unwrap_or(Value::Null);
            let reason = match error {
                LeaseError::Unauthorized { reason }
                | LeaseError::InvalidSpec { reason }
                | LeaseError::Backend { message: reason } => {
                    Some(redact(reason.as_bytes(), secrets).chars().take(256).collect::<String>())
                }
                _ => None,
            };
            json!({"outcome":"denied", "kind":encoded.get("kind").and_then(Value::as_str), "reason":reason})
        }
        LeaseResponse::Granted(_) => json!({"outcome":"unexpected_grant"}),
    }
}

/// The path of a sibling binary built into the same target profile directory as the worker under test.
pub(super) fn binary(name: &str) -> anyhow::Result<PathBuf> {
    let path = Path::new(env!("CARGO_BIN_EXE_trogon-sandbox-docker-worker"))
        .parent()
        .context("worker binary has no profile directory")?
        .join(name);
    ensure!(
        path.is_file(),
        "build the current broker binary first: cargo build -p trogon-sandbox-broker"
    );
    Ok(path)
}

pub(super) fn canonical(id: &str) -> bool {
    id.len() == 64 && id.bytes().all(|byte| byte.is_ascii_hexdigit())
}

pub(super) fn inherit_runtime(command: &mut Command) {
    command.env_clear();
    for name in [
        "PATH",
        "HOME",
        "TMPDIR",
        "DOCKER_HOST",
        "DOCKER_CONTEXT",
        "DOCKER_TLS_VERIFY",
        "DOCKER_CERT_PATH",
        "DOCKER_CONFIG",
    ] {
        if let Some(value) = trogon_std::env::ReadEnv::var_os(&trogon_std::env::SystemEnv, name) {
            command.env(name, value);
        }
    }
}
