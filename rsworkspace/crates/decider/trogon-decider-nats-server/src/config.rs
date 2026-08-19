//! Resolving the host's configuration from flags and the environment.
//!
//! Every value that can be wrong is turned into its value object here, at
//! startup, rather than at the first request that needs it. A prefix that
//! cannot be a subject or a limit of zero is a deployment mistake, and a
//! deployment mistake that surfaces as a per-command fault is one that reaches
//! production.

use std::path::PathBuf;
use std::str::FromStr;
use std::time::Duration;

use clap::Parser;
use trogon_decider_nats::{DuplicateWindow, InvalidDuplicateWindowError};
use trogon_decider_runtime::{AdmissionLimit, ReplayLimit};
use trogon_nats::{NatsConfig, SubjectTokenViolationError};
use trogon_std::ParseArgs;
use trogon_std::env::ReadEnv;

use crate::constants::{
    DEFAULT_ADMISSION_LIMIT, DEFAULT_DUPLICATE_WINDOW_SECS, DEFAULT_EVENTS_STREAM, DEFAULT_MODULE_BUCKET,
    DEFAULT_QUEUE_GROUP, DEFAULT_SNAPSHOT_BUCKET, DEFAULT_SUBJECT_PREFIX, ENV_DECIDER_ADMISSION_LIMIT,
    ENV_DECIDER_DUPLICATE_WINDOW_SECS, ENV_DECIDER_EVENTS_STREAM, ENV_DECIDER_MODULE_STORE, ENV_DECIDER_MODULES,
    ENV_DECIDER_QUEUE_GROUP, ENV_DECIDER_REPLAY_LIMIT, ENV_DECIDER_SNAPSHOT_BUCKET, ENV_DECIDER_SUBJECT_PREFIX,
    MODULE_STORE_DIRECTORY_SCHEME, MODULE_STORE_OBJECT_SCHEME,
};
use crate::endpoint::{CommandEndpoint, CommandEndpointError, SubjectPrefix};
use crate::module_reference::{ModuleReference, ModuleReferenceError};

#[derive(Parser, Debug, Clone)]
#[command(name = "trogon-decider-nats-server")]
#[command(about = "NATS host for WASM decider modules", long_about = None)]
pub struct Args {
    #[arg(long = "subject-prefix")]
    pub subject_prefix: Option<String>,
    #[arg(long = "queue-group")]
    pub queue_group: Option<String>,
    #[arg(long = "events-stream")]
    pub events_stream: Option<String>,
    #[arg(long = "snapshot-bucket")]
    pub snapshot_bucket: Option<String>,
    #[arg(long = "module-store")]
    pub module_store: Option<String>,
    #[arg(long = "module")]
    pub modules: Vec<String>,
    #[arg(long = "admission-limit")]
    pub admission_limit: Option<usize>,
    #[arg(long = "replay-limit")]
    pub replay_limit: Option<u64>,
    #[arg(long = "duplicate-window-secs")]
    pub duplicate_window_secs: Option<u64>,
}

/// Everything the host needs to serve commands.
#[derive(Debug, Clone)]
pub struct ServerConfig {
    /// The command subject projection this host answers under.
    pub endpoint: CommandEndpoint,
    /// Queue group the host's replicas share.
    pub queue_group: String,
    /// JetStream stream holding decider events.
    pub events_stream: String,
    /// Key/Value bucket holding decider snapshots.
    pub snapshot_bucket: String,
    /// Store the host resolves its module references against, per ADR#0058.
    pub module_store: ModuleStore,
    /// Modules to activate at startup, named as `{name}@{version}`.
    pub modules: Vec<ModuleReference>,
    /// How many commands may execute concurrently, per ADR#0028.
    pub admission_limit: AdmissionLimit,
    /// How many events one command may replay, if bounded.
    pub replay_limit: Option<ReplayLimit>,
    /// How long JetStream remembers an event id, bounding command idempotency.
    pub duplicate_window: DuplicateWindow,
}

/// Resolves the host config and the NATS connection it runs over.
pub fn base_config<P: ParseArgs<Args = Args>, E: ReadEnv>(
    parser: &P,
    env: &E,
) -> Result<(ServerConfig, NatsConfig), ConfigError> {
    config_from_args(parser.parse_args(), env)
}

fn config_from_args<E: ReadEnv>(args: Args, env: &E) -> Result<(ServerConfig, NatsConfig), ConfigError> {
    let subject_prefix = args
        .subject_prefix
        .or_else(|| env.var(ENV_DECIDER_SUBJECT_PREFIX).ok())
        .unwrap_or_else(|| DEFAULT_SUBJECT_PREFIX.to_owned());
    let queue_group = args
        .queue_group
        .or_else(|| env.var(ENV_DECIDER_QUEUE_GROUP).ok())
        .unwrap_or_else(|| DEFAULT_QUEUE_GROUP.to_owned());
    let events_stream = args
        .events_stream
        .or_else(|| env.var(ENV_DECIDER_EVENTS_STREAM).ok())
        .unwrap_or_else(|| DEFAULT_EVENTS_STREAM.to_owned());
    let snapshot_bucket = args
        .snapshot_bucket
        .or_else(|| env.var(ENV_DECIDER_SNAPSHOT_BUCKET).ok())
        .unwrap_or_else(|| DEFAULT_SNAPSHOT_BUCKET.to_owned());

    let module_store = args
        .module_store
        .or_else(|| env.var(ENV_DECIDER_MODULE_STORE).ok())
        .map_or_else(
            || Ok(ModuleStore::ObjectStore(DEFAULT_MODULE_BUCKET.to_owned())),
            |value| value.parse(),
        )?;

    let written = if args.modules.is_empty() {
        split_list(env.var(ENV_DECIDER_MODULES).ok().as_deref())
    } else {
        args.modules
    };
    if written.is_empty() {
        return Err(ConfigError::NoModules);
    }
    let modules = written
        .iter()
        .map(|value| value.parse::<ModuleReference>())
        .collect::<Result<Vec<_>, _>>()?;

    let admission_limit = args
        .admission_limit
        .or(parse_env(env, ENV_DECIDER_ADMISSION_LIMIT)?)
        .unwrap_or(DEFAULT_ADMISSION_LIMIT);
    let admission_limit = AdmissionLimit::try_new(admission_limit).map_err(|_| ConfigError::AdmissionLimitZero)?;

    let replay_limit = args
        .replay_limit
        .or(parse_env(env, ENV_DECIDER_REPLAY_LIMIT)?)
        .map(|limit| ReplayLimit::try_new(limit).map_err(|_| ConfigError::ReplayLimitZero))
        .transpose()?;

    let duplicate_window_secs = args
        .duplicate_window_secs
        .or(parse_env(env, ENV_DECIDER_DUPLICATE_WINDOW_SECS)?)
        .unwrap_or(DEFAULT_DUPLICATE_WINDOW_SECS);
    let duplicate_window = DuplicateWindow::try_new(Duration::from_secs(duplicate_window_secs)).map_err(|source| {
        ConfigError::DuplicateWindow {
            seconds: duplicate_window_secs,
            source,
        }
    })?;

    let config = ServerConfig {
        endpoint: CommandEndpoint::new(SubjectPrefix::new(&subject_prefix).map_err(|source| {
            ConfigError::SubjectPrefix {
                value: subject_prefix.clone(),
                source,
            }
        })?)?,
        queue_group,
        events_stream,
        snapshot_bucket,
        module_store,
        modules,
        admission_limit,
        replay_limit,
        duplicate_window,
    };

    Ok((config, NatsConfig::from_env(env)))
}

fn split_list(value: Option<&str>) -> Vec<String> {
    value
        .into_iter()
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Which store a host resolves its module references against.
///
/// Written `objectstore:BUCKET` or `file:/path/to/dir`. The scheme is required
/// rather than inferred from the shape of the value, because a bucket name and
/// a relative directory are the same string and guessing between them would
/// make a typo silently change which store is searched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModuleStore {
    /// A JetStream object store bucket, per ADR#0058.
    ObjectStore(String),
    /// A local directory, for development and tests.
    Directory(PathBuf),
}

impl FromStr for ModuleStore {
    type Err = ConfigError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let Some((scheme, rest)) = value.split_once(':') else {
            return Err(ConfigError::ModuleStoreScheme {
                value: value.to_owned(),
            });
        };
        match scheme {
            MODULE_STORE_OBJECT_SCHEME => {
                if rest.is_empty() || !rest.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
                    return Err(ConfigError::ModuleBucketName { value: rest.to_owned() });
                }
                Ok(Self::ObjectStore(rest.to_owned()))
            }
            MODULE_STORE_DIRECTORY_SCHEME if !rest.is_empty() => Ok(Self::Directory(PathBuf::from(rest))),
            _ => Err(ConfigError::ModuleStoreScheme {
                value: value.to_owned(),
            }),
        }
    }
}

impl std::fmt::Display for ModuleStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ObjectStore(bucket) => write!(f, "{MODULE_STORE_OBJECT_SCHEME}:{bucket}"),
            Self::Directory(root) => write!(f, "{MODULE_STORE_DIRECTORY_SCHEME}:{}", root.display()),
        }
    }
}

fn parse_env<E, T>(env: &E, name: &'static str) -> Result<Option<T>, ConfigError>
where
    E: ReadEnv,
    T: std::str::FromStr,
{
    match env.var(name) {
        Ok(value) => value
            .parse()
            .map(Some)
            .map_err(|_| ConfigError::NotANumber { name, value }),
        Err(_) => Ok(None),
    }
}

/// Why the host cannot start with the configuration it was given.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// The configured subject prefix is not a NATS token.
    #[error("subject prefix '{value}' is not a valid NATS token: {source}")]
    SubjectPrefix {
        value: String,
        #[source]
        source: SubjectTokenViolationError,
    },
    /// The configured prefix does not yield a usable endpoint subject.
    #[error(transparent)]
    Endpoint(#[from] CommandEndpointError),
    /// No module was configured, so the host would answer nothing.
    #[error("no decider module configured; set --module or {ENV_DECIDER_MODULES}")]
    NoModules,
    /// A configured module is not a `{name}@{version}` reference.
    #[error(transparent)]
    ModuleReference(#[from] ModuleReferenceError),
    /// The module store was written without a scheme this host understands.
    #[error(
        "module store '{value}' must be '{MODULE_STORE_OBJECT_SCHEME}:BUCKET' or '{MODULE_STORE_DIRECTORY_SCHEME}:/path/to/dir'"
    )]
    ModuleStoreScheme { value: String },
    /// The bucket name is not one JetStream will accept.
    #[error("module bucket '{value}' is not alphanumeric, '_', or '-'")]
    ModuleBucketName { value: String },
    /// A numeric setting is not a number.
    #[error("{name} '{value}' is not an unsigned integer")]
    NotANumber { name: &'static str, value: String },
    /// An admission limit of zero admits nothing.
    #[error("{ENV_DECIDER_ADMISSION_LIMIT} is zero, which would shed every command")]
    AdmissionLimitZero,
    /// A replay limit of zero replays nothing.
    #[error("{ENV_DECIDER_REPLAY_LIMIT} is zero, which would fail every command with history")]
    ReplayLimitZero,
    /// The duplicate window is one JetStream will not enforce.
    #[error("{ENV_DECIDER_DUPLICATE_WINDOW_SECS} of {seconds}s is not enforceable: {source}")]
    DuplicateWindow {
        seconds: u64,
        #[source]
        source: InvalidDuplicateWindowError,
    },
}

#[cfg(test)]
mod tests;
