use std::collections::VecDeque;
use std::future::Future;
use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::{FutureExt, StreamExt};
use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::{
    ContainerAsync, ContainerRequest, ImageExt, TestcontainersError,
    bollard::{
        Docker,
        container::LogOutput,
        models::ContainerInspectResponse,
        query_parameters::{InspectContainerOptionsBuilder, LogsOptionsBuilder},
    },
    core::{
        client::{ClientError, docker_client_instance},
        error::WaitContainerError,
        logs::LogFrame,
    },
    runners::AsyncRunner,
};
use tokio::time::{Instant, timeout};

mod constants;
mod fixture_observation;

use fixture_observation::{CollectionOperation, CollectionOutcome, FixtureObservation, FixtureRecord, StartupOutcome};

#[derive(Clone)]
pub(crate) struct FixtureDiagnostics {
    name: String,
    image: &'static str,
    started: Instant,
    evidence: Arc<Mutex<Evidence>>,
    live_stderr: Arc<Mutex<StderrSelection>>,
}

struct Evidence {
    records: VecDeque<String>,
    container_id: Option<String>,
    truncated: bool,
}

struct StderrSelection {
    pending: Vec<u8>,
    discard_line: bool,
    remaining: usize,
    truncated: bool,
}

impl FixtureDiagnostics {
    pub(crate) fn for_request(request: &ContainerRequest<Nats>) -> Option<Self> {
        let image = match request.descriptor().as_str() {
            "nats:2.10.14" => "nats:2.10.14",
            "nats:2.14.2" => "nats:2.14.2",
            _ => return None,
        };
        let mut command = request.cmd();
        if command.next().as_deref() != Some("--jetstream")
            || command.next().is_some()
            || request.env_vars().next().is_some()
            || request.copy_to_sources().next().is_some()
            || request.mounts().next().is_some()
            || request.entrypoint().is_some()
            || request.container_name().is_some()
        {
            return None;
        }
        Some(Self {
            name: format!("nats-fixture-{}", uuid::Uuid::new_v4().simple()),
            image,
            started: Instant::now(),
            evidence: Arc::new(Mutex::new(Evidence {
                records: VecDeque::new(),
                container_id: None,
                truncated: false,
            })),
            live_stderr: Arc::new(Mutex::new(StderrSelection::new())),
        })
    }

    pub(crate) async fn start(&self, request: ContainerRequest<Nats>) -> ContainerAsync<Nats> {
        let consumer = self.clone();
        let request = request
            .with_container_name(&self.name)
            .with_label(constants::LABEL, &self.name)
            .with_log_consumer(move |frame: &LogFrame| {
                if let LogFrame::StdErr(bytes) = frame {
                    consumer.stderr(bytes);
                }
            });
        let mut guard = StartupEvidence {
            diagnostics: self,
            completed: false,
        };
        let result = tokio::select! {
            result = request.start() => result,
            () = self.observe_startup(&docker_client_instance) => unreachable!("observer remains pending"),
        };
        match result {
            Ok(container) => {
                guard.completed = true;
                container
            }
            Err(error) => {
                self.record(FixtureObservation::StartupResult {
                    startup_result: startup_error(&error),
                    client_phase: "connection_not_attempted",
                });
                panic!("start NATS testcontainer failed; retained diagnostics follow");
            }
        }
    }

    fn observe_startup<F>(&self, connect: &impl Fn() -> F) -> impl Future<Output = ()>
    where
        F: Future<Output = Result<Docker, ClientError>>,
    {
        // Snapshots race alongside the original runner future, so collection cannot
        // defer polling its timeout. These are request-relative observations, not phase timings.
        async {
            for seconds in [1, 30, 55] {
                tokio::time::sleep_until(self.started + Duration::from_secs(seconds)).await;
                if timeout(constants::COLLECTION_BUDGET, self.snapshot(connect()))
                    .await
                    .is_err()
                {
                    self.record(FixtureObservation::Collection {
                        operation: CollectionOperation::StartupSnapshot,
                        outcome: CollectionOutcome::CollectionTimeout,
                    });
                }
            }
        }
        .then(|()| std::future::pending())
    }

    pub(crate) async fn capture(&self, container: &ContainerAsync<Nats>) -> String {
        self.capture_owned(container.id(), &docker_client_instance).await
    }

    async fn capture_owned<F>(&self, id: &str, connect: &impl Fn() -> F) -> String
    where
        F: Future<Output = Result<Docker, ClientError>>,
    {
        if is_hex(id, 64) {
            self.evidence.lock().unwrap().container_id = Some(id.to_owned());
            self.record(FixtureObservation::ContainerHandle { container_id: id });
        }
        let collection = async {
            let docker = match connect().await {
                Ok(docker) => docker,
                Err(_) => {
                    self.record(FixtureObservation::Collection {
                        operation: CollectionOperation::DockerClient,
                        outcome: CollectionOutcome::Unavailable,
                    });
                    return;
                }
            };
            // Read the warning first while this caller still owns the live broker.
            self.logs(&docker, id).await;
            self.snapshot(connect()).await;
        };
        if timeout(constants::COLLECTION_BUDGET, collection).await.is_err() {
            self.record(FixtureObservation::Collection {
                operation: CollectionOperation::FailureCollection,
                outcome: CollectionOutcome::CollectionTimeout,
            });
        }
        self.report()
    }

    async fn snapshot(&self, connection: impl Future<Output = Result<Docker, ClientError>>) {
        let docker = match connection.await {
            Ok(docker) => docker,
            Err(_) => {
                self.record(FixtureObservation::Collection {
                    operation: CollectionOperation::DockerClient,
                    outcome: CollectionOutcome::Unavailable,
                });
                return;
            }
        };
        let options = InspectContainerOptionsBuilder::new().size(true).build();
        let inspected = match docker.inspect_container(&self.name, Some(options)).await {
            Ok(inspected) => inspected,
            Err(_) => {
                self.record(FixtureObservation::Collection {
                    operation: CollectionOperation::ContainerInspect,
                    outcome: CollectionOutcome::Unavailable,
                });
                return;
            }
        };
        let owned = inspected
            .name
            .as_deref()
            .map(|name| name.strip_prefix('/').unwrap_or(name))
            == Some(self.name.as_str())
            && inspected.config.as_ref().and_then(|config| config.image.as_deref()) == Some(self.image)
            && inspected
                .config
                .as_ref()
                .and_then(|config| config.labels.as_ref())
                .and_then(|labels| labels.get(constants::LABEL))
                == Some(&self.name);
        if !owned {
            self.record(FixtureObservation::Collection {
                operation: CollectionOperation::ContainerInspect,
                outcome: CollectionOutcome::OwnershipMismatch,
            });
            return;
        }
        let id = inspected.id.as_deref().filter(|id| is_hex(id, 64));
        if let Some(id) = id {
            self.evidence.lock().unwrap().container_id = Some(id.to_owned());
        }
        self.record(selected_container_state(&inspected));
        if let Some(id) = id {
            self.logs(&docker, id).await;
        }
        let Some(image_id) = inspected.image.as_deref().filter(|id| digest(id).is_some()) else {
            return;
        };
        match docker.inspect_image(image_id).await {
            Ok(image) => self.record(FixtureObservation::Image {
                image_id,
                architecture: image.architecture.as_deref().filter(|value| {
                    matches!(
                        *value,
                        "amd64" | "arm64" | "arm" | "386" | "ppc64le" | "s390x" | "riscv64"
                    )
                }),
                os: image
                    .os
                    .as_deref()
                    .filter(|value| matches!(*value, "linux" | "windows")),
                repo_digests: image
                    .repo_digests
                    .iter()
                    .flatten()
                    .filter_map(|value| {
                        let (repository, hash) = value.split_once('@')?;
                        matches!(repository, "nats" | "docker.io/library/nats")
                            .then(|| digest(hash))
                            .flatten()
                    })
                    .take(2)
                    .collect(),
            }),
            Err(_) => self.record(FixtureObservation::Collection {
                operation: CollectionOperation::ImageInspect,
                outcome: CollectionOutcome::Unavailable,
            }),
        }
    }

    async fn logs(&self, docker: &Docker, id: &str) {
        let options = LogsOptionsBuilder::new().stderr(true).follow(false).tail("128").build();
        let mut logs = docker.logs(id, Some(options)).take(128);
        // Each finite tail read gets its own byte budget, independent of the live consumer.
        let mut selection = StderrSelection::new();
        while let Some(frame) = logs.next().await {
            match frame {
                Ok(LogOutput::StdErr { message }) => {
                    self.selected_stderr(&mut selection, &message);
                    if selection.remaining == 0 {
                        break;
                    }
                }
                Ok(_) => {}
                Err(_) => {
                    self.record(FixtureObservation::Collection {
                        operation: CollectionOperation::StderrTail,
                        outcome: CollectionOutcome::Unavailable,
                    });
                    break;
                }
            }
        }
    }

    fn stderr(&self, bytes: &[u8]) {
        self.selected_stderr(&mut self.live_stderr.lock().unwrap(), bytes);
    }

    fn selected_stderr(&self, selection: &mut StderrSelection, bytes: &[u8]) {
        for selected in selection.feed(bytes) {
            self.record(FixtureObservation::BrokerStderr {
                broker_stderr: &selected,
            });
        }
        if selection.truncated {
            self.evidence.lock().unwrap().truncated = true;
        }
    }

    fn record(&self, observation: FixtureObservation<'_>) {
        self.push(&mut self.evidence.lock().unwrap(), observation);
    }

    fn push(&self, evidence: &mut Evidence, observation: FixtureObservation<'_>) {
        if evidence.records.len() == constants::MAX_RECORDS {
            evidence.records.pop_front();
            evidence.truncated = true;
        }
        let record = FixtureRecord {
            fixture: &self.name,
            requested_image: self.image,
            request_elapsed_ms: self.started.elapsed().as_millis(),
            observation,
        };
        evidence
            .records
            .push_back(serde_json::to_string(&record).expect("serialize selected fixture evidence"));
    }

    fn report(&self) -> String {
        let evidence = self.evidence.lock().unwrap();
        let mut report = format!(
            "nats-fixture={} requested_image={} diagnostics_truncated={}\n",
            self.name, self.image, evidence.truncated
        );
        if let Some(id) = &evidence.container_id {
            report.push_str(&format!("owned-container={id}\n"));
        }
        for record in &evidence.records {
            report.push_str(record);
            report.push('\n');
        }
        report
    }
}

impl StderrSelection {
    fn new() -> Self {
        Self {
            pending: Vec::new(),
            discard_line: false,
            remaining: constants::MAX_INPUT,
            truncated: false,
        }
    }

    fn feed(&mut self, bytes: &[u8]) -> VecDeque<String> {
        let take = bytes.len().min(self.remaining);
        self.remaining -= take;
        if take < bytes.len() {
            self.truncated = true;
        }
        let mut selected = VecDeque::new();
        for &byte in &bytes[..take] {
            if byte == b'\n' {
                if !self.discard_line
                    && let Ok(line) = std::str::from_utf8(&self.pending)
                    && let Some(message) = selected_log(line)
                {
                    if selected.len() == constants::MAX_RECORDS {
                        selected.pop_front();
                        self.truncated = true;
                    }
                    selected.push_back(message);
                }
                self.pending.clear();
                self.discard_line = false;
            } else if self.pending.len() < constants::MAX_LINE && !self.discard_line {
                self.pending.push(byte);
            } else {
                self.pending.clear();
                self.discard_line = true;
                self.truncated = true;
            }
        }
        selected
    }
}

struct StartupEvidence<'a> {
    diagnostics: &'a FixtureDiagnostics,
    completed: bool,
}

impl Drop for StartupEvidence<'_> {
    fn drop(&mut self) {
        if !self.completed {
            // Test harness output must survive without a tracing subscriber, and an I/O
            // failure during unwinding must not replace the original fixture failure.
            let _ = writeln!(
                std::io::stderr().lock(),
                "startup_future_did_not_return_container; connection_not_attempted\n{}",
                self.diagnostics.report()
            );
        }
    }
}

fn startup_error(error: &TestcontainersError) -> StartupOutcome {
    match error {
        TestcontainersError::WaitContainer(WaitContainerError::StartupTimeout) => StartupOutcome::StartupTimeout,
        TestcontainersError::WaitContainer(_) => StartupOutcome::ReadinessError,
        TestcontainersError::Client(_) => StartupOutcome::DockerClientError,
        TestcontainersError::PortNotExposed { .. } => StartupOutcome::PortNotExposed,
        TestcontainersError::MissingInfo(_) => StartupOutcome::MissingContainerInfo,
        TestcontainersError::Exec(_) => StartupOutcome::ExecError,
        TestcontainersError::Io(_) => StartupOutcome::IoError,
        TestcontainersError::Other(_) => StartupOutcome::OtherError,
    }
}

fn selected_container_state(inspected: &ContainerInspectResponse) -> FixtureObservation<'_> {
    let state = inspected.state.as_ref();
    let state_error = state.and_then(|state| state.error.as_deref());
    FixtureObservation::ContainerState {
        container_id: inspected.id.as_deref().filter(|id| is_hex(id, 64)),
        image_id: inspected.image.as_deref().filter(|id| digest(id).is_some()),
        state: state.and_then(|state| state.status),
        running: state.and_then(|state| state.running),
        oom_killed: state.and_then(|state| state.oom_killed),
        exit_code: state.and_then(|state| state.exit_code),
        state_error_present: state_error.map(|error| !error.is_empty()),
        state_error_selected: state_error.and_then(safe_error),
        size_rw: inspected.size_rw,
        size_root_fs: inspected.size_root_fs,
    }
}

fn is_hex(value: &str, length: usize) -> bool {
    value.len() == length && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn digest(value: &str) -> Option<&str> {
    is_hex(value.strip_prefix("sha256:")?, 64).then_some(value)
}

fn selected_log(line: &str) -> Option<String> {
    let (prefix, record) = line.split_once(" [")?;
    let (pid, timestamp) = prefix.strip_prefix('[')?.split_once("] ")?;
    if pid.is_empty()
        || !pid.bytes().all(|byte| byte.is_ascii_digit())
        || timestamp.is_empty()
        || !timestamp
            .bytes()
            .all(|byte| byte.is_ascii_digit() || b"/ :.".contains(&byte))
    {
        return None;
    }
    let (level, message) = record.split_once("] ")?;
    if level == "INF" {
        let message = message.trim();
        return match message {
            "Starting nats-server"
            | "Starting JetStream"
            | "Listening for client connections on 0.0.0.0:4222"
            | "Server is ready"
            | "Store Directory: \"/tmp/nats/jetstream\"" => Some(message.to_owned()),
            _ => None,
        };
    }
    if !matches!(level, "WRN" | "ERR" | "FTL") {
        return None;
    }
    if let Some(error) = message.strip_prefix("Stream create failed for '$G > AUDIT': ") {
        // NATS formats its internal store error with this numeric code before
        // replacing it with the less specific client-facing 10049.
        let (error, code) = match error.strip_suffix(" (10077)") {
            Some(error) => (error, " (10077)"),
            None => (error, ""),
        };
        return Some(format!(
            "Stream create failed for '$G > AUDIT': {}{code}",
            safe_error(error).as_deref().unwrap_or("unrecognized detail suppressed")
        ));
    }
    Some(format!(
        "broker {level} observed: {}",
        safe_error(message)
            .as_deref()
            .unwrap_or("unrecognized detail suppressed")
    ))
}

// Only complete, known broker/OS messages survive. Caller-controlled names, paths,
// descriptions, configuration fragments, and unknown error text never enter the report.
fn safe_error(error: &str) -> Option<String> {
    match error {
        "storage directory is not a directory" | "storage directory is not writable" => return Some(error.to_owned()),
        _ => {}
    }
    if let Some(error) = os_error(error) {
        return Some(error.to_owned());
    }
    for context in [
        "could not create storage directory - ",
        "could not create message storage directory - ",
        "could not create consumer storage directory - ",
        "",
    ] {
        let Some(detail) = error.strip_prefix(context) else {
            continue;
        };
        let Some((operation_and_path, cause)) = detail.rsplit_once(": ") else {
            continue;
        };
        let Some(cause) = os_error(cause) else {
            continue;
        };
        let Some((operation, path)) = operation_and_path.split_once(' ') else {
            continue;
        };
        if matches!(
            operation,
            "mkdir" | "open" | "stat" | "lstat" | "read" | "write" | "rename" | "remove"
        ) && path.starts_with("/tmp/nats/jetstream/")
        {
            return Some(format!("{context}{operation} <fixture-store-path>: {cause}"));
        }
    }
    None
}

fn os_error(error: &str) -> Option<&'static str> {
    match error {
        "no space left on device" => Some("no space left on device"),
        "permission denied" => Some("permission denied"),
        "no such file or directory" => Some("no such file or directory"),
        "not a directory" => Some("not a directory"),
        "file exists" => Some("file exists"),
        "input/output error" => Some("input/output error"),
        "read-only file system" => Some("read-only file system"),
        "too many open files" => Some("too many open files"),
        "too many open files in system" => Some("too many open files in system"),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
