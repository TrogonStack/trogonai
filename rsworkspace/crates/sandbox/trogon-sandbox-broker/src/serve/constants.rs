use std::time::Duration;

/// Service name the broker reports to tracing and telemetry.
pub const SERVICE_NAME: &str = "trogon-sandbox-broker";

pub(super) const SNAPSHOT_ACCOUNTANTS_QUEUE: &str = "sandbox-snapshot-accountants";

pub(super) const PUBLICATION_RECOVERY_INTERVAL: Duration = Duration::from_secs(5);

pub(super) const OPERATIONS_DRIVE_INTERVAL: Duration = Duration::from_secs(10);

pub(super) const LAUNCHES_BUCKET_SUFFIX: &str = "-launches";

pub(super) const OPERATION_OWNER_PREFIX: &str = "broker-";

pub(super) const LEASE_CANCEL_PREFIX: &str = "sandbox.lease.cancel.";

pub(super) const WRITE_FILES_PREFIX: &str = "sandbox.lease.write_files.";

pub(super) const READ_FILE_PREFIX: &str = "sandbox.lease.read_file.";

pub(super) const INSECURE_DEV_WARNING: &str = "INSECURE DEV MODE: bearer authentication, the image allow-list and \
     the worker registry are all disabled; anyone who can reach NATS can lease any image. Never use \
     --insecure-dev outside a local development stack";
