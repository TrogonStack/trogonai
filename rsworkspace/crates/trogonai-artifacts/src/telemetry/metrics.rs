use std::sync::OnceLock;

use opentelemetry::metrics::Counter;
use opentelemetry::{KeyValue, global};

static ARTIFACT_STORED: OnceLock<Counter<u64>> = OnceLock::new();
static ARTIFACT_RETRIEVED: OnceLock<Counter<u64>> = OnceLock::new();
static ARTIFACT_CHECKSUM_MISMATCH: OnceLock<Counter<u64>> = OnceLock::new();
static ARTIFACT_UNAVAILABLE: OnceLock<Counter<u64>> = OnceLock::new();

fn meter() -> opentelemetry::metrics::Meter {
    global::meter("trogonai-artifacts")
}

fn artifact_stored_counter() -> &'static Counter<u64> {
    ARTIFACT_STORED.get_or_init(|| {
        meter()
            .u64_counter("trogonai.artifacts.store")
            .with_description("Session artifacts stored inline or via claim-check")
            .build()
    })
}

fn artifact_retrieved_counter() -> &'static Counter<u64> {
    ARTIFACT_RETRIEVED.get_or_init(|| {
        meter()
            .u64_counter("trogonai.artifacts.retrieve")
            .with_description("Session artifacts retrieved from object store")
            .build()
    })
}

fn artifact_checksum_mismatch_counter() -> &'static Counter<u64> {
    ARTIFACT_CHECKSUM_MISMATCH.get_or_init(|| {
        meter()
            .u64_counter("trogonai.artifacts.checksum_mismatch")
            .with_description("Artifact retrieval failed checksum verification")
            .build()
    })
}

fn artifact_unavailable_counter() -> &'static Counter<u64> {
    ARTIFACT_UNAVAILABLE.get_or_init(|| {
        meter()
            .u64_counter("trogonai.artifacts.unavailable")
            .with_description("Referenced artifact could not be retrieved (artifact_unavailable)")
            .build()
    })
}

pub fn record_artifact_stored(session_id: &str, storage_mode: &str, size_bytes: u64) {
    artifact_stored_counter().add(
        1,
        &[
            KeyValue::new("session_id", session_id.to_string()),
            KeyValue::new("storage_mode", storage_mode.to_string()),
            KeyValue::new("size_bytes", size_bytes.to_string()),
        ],
    );
}

pub fn record_artifact_retrieved(session_id: &str, artifact_id: &str) {
    artifact_retrieved_counter().add(
        1,
        &[
            KeyValue::new("session_id", session_id.to_string()),
            KeyValue::new("artifact_id", artifact_id.to_string()),
        ],
    );
}

pub fn record_checksum_mismatch(session_id: &str, artifact_id: &str) {
    artifact_checksum_mismatch_counter().add(
        1,
        &[
            KeyValue::new("session_id", session_id.to_string()),
            KeyValue::new("artifact_id", artifact_id.to_string()),
        ],
    );
}

pub fn record_artifact_unavailable(session_id: &str, artifact_id: &str, reason: &str) {
    artifact_unavailable_counter().add(
        1,
        &[
            KeyValue::new("session_id", session_id.to_string()),
            KeyValue::new("artifact_id", artifact_id.to_string()),
            KeyValue::new("reason", reason.to_string()),
        ],
    );
}
