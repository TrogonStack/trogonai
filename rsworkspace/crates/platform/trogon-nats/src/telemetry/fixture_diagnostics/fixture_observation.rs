use serde::Serialize;
use testcontainers_modules::testcontainers::bollard::models::ContainerStateStatusEnum;

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(super) enum FixtureObservation<'a> {
    StartupResult {
        startup_result: StartupOutcome,
        client_phase: &'static str,
    },
    Collection {
        operation: CollectionOperation,
        outcome: CollectionOutcome,
    },
    ContainerHandle {
        container_id: &'a str,
    },
    ContainerState {
        container_id: Option<&'a str>,
        image_id: Option<&'a str>,
        state: Option<ContainerStateStatusEnum>,
        running: Option<bool>,
        oom_killed: Option<bool>,
        exit_code: Option<i64>,
        state_error_present: Option<bool>,
        state_error_selected: Option<String>,
        size_rw: Option<i64>,
        size_root_fs: Option<i64>,
    },
    Image {
        image_id: &'a str,
        architecture: Option<&'a str>,
        os: Option<&'a str>,
        repo_digests: Vec<&'a str>,
    },
    BrokerStderr {
        broker_stderr: &'a str,
    },
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum StartupOutcome {
    StartupTimeout,
    ReadinessError,
    DockerClientError,
    PortNotExposed,
    MissingContainerInfo,
    ExecError,
    IoError,
    OtherError,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum CollectionOperation {
    DockerClient,
    ContainerInspect,
    ImageInspect,
    StderrTail,
    StartupSnapshot,
    FailureCollection,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum CollectionOutcome {
    Unavailable,
    CollectionTimeout,
    OwnershipMismatch,
}

#[derive(Serialize)]
pub(super) struct FixtureRecord<'a> {
    pub(super) fixture: &'a str,
    pub(super) requested_image: &'a str,
    pub(super) request_elapsed_ms: u128,
    pub(super) observation: FixtureObservation<'a>,
}
