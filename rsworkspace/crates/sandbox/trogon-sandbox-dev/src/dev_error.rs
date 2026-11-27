use std::path::PathBuf;
use std::time::Duration;

use trogon_sandbox_auth::{ApiKeyStoreError, BearerTokenError};

use crate::PlanError;

/// A step of starting or supervising the local stack failed.
#[derive(Debug, thiserror::Error)]
pub enum DevError {
    #[error(transparent)]
    Plan(#[from] PlanError),
    #[error("create scratch directory: {0}")]
    ScratchDirectory(#[source] std::io::Error),
    #[error("the supplied API key is not a valid bearer token: {0}")]
    InvalidApiKey(#[from] BearerTokenError),
    #[error("install the shutdown signal handler: {0}")]
    SignalHandler(#[source] std::io::Error),
    #[error("generate an API key: {0}")]
    GenerateApiKey(#[source] rand_core::Error),
    #[error("create private API key file {path}: {source}")]
    ApiKeyFile {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("spawn {name} ({program}): {source}")]
    Spawn {
        name: &'static str,
        program: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{service} did not accept connections on {addr} within {timeout:?}{hint}")]
    NotListening {
        service: &'static str,
        addr: String,
        timeout: Duration,
        hint: &'static str,
    },
    #[error("connect NATS: {0}")]
    NatsConnect(#[source] async_nats::ConnectError),
    #[error("register the dev API key: {0}")]
    RegisterApiKey(#[source] ApiKeyStoreError),
    #[error("store the dev API key: {0}")]
    StoreApiKey(#[source] async_nats::jetstream::kv::PutError),
    #[error("encode the dev API key record: {0}")]
    EncodeApiKey(#[source] serde_json::Error),
    #[error("broker did not serve tenant limits before the startup deadline")]
    BrokerDeadline,
    #[error("request broker tenant limits during startup: {0}")]
    BrokerRequest(#[source] async_nats::RequestError),
    #[error("encode the broker tenant limits request: {0}")]
    EncodeBrokerRequest(#[source] serde_json::Error),
    #[error("broker returned an invalid tenant limits response: {0}")]
    InvalidBrokerResponse(#[source] serde_json::Error),
    #[error("broker refused tenant limits during startup")]
    BrokerRefused,
    #[error("{name} exited ({status}) before the stack was ready")]
    ExitedDuringStartup { name: &'static str, status: String },
}
