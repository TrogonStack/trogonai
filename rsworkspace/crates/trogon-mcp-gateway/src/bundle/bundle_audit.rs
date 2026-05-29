//! Bundle loader audit events via the gateway JetStream audit publisher.

use std::time::Duration;

use async_nats::jetstream;
use serde::Serialize;

use std::sync::Arc;
use tokio::sync::Mutex;

use crate::audit::{self, AuditEnvelope};
use crate::authz::IdentitySource;

use super::errors::BundleLoadError;

pub const EVENT_LOAD_SUCCEEDED: &str = "bundle.load_succeeded";
pub const EVENT_LOAD_FAILED: &str = "bundle.load_failed";
pub const EVENT_HOT_SWAPPED: &str = "bundle.hot_swapped";
pub const EVENT_ROLLED_BACK: &str = "bundle.rolled_back";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BundleAuditFields {
    pub bundle_id: String,
    pub revision: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_class: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policy_bundle_digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_digest: Option<String>,
}

#[derive(Debug, Clone)]
pub enum BundleAuditBackend {
    JetStream {
        jetstream: jetstream::Context,
        prefix: String,
    },
    Recording(Arc<Mutex<Vec<RecordedBundleAudit>>>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedBundleAudit {
    pub event: &'static str,
    pub fields: BundleAuditFields,
}

#[derive(Clone)]
pub struct BundleAuditPublisher {
    backend: BundleAuditBackend,
}

impl std::fmt::Debug for BundleAuditPublisher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("BundleAuditPublisher")
    }
}

impl BundleAuditPublisher {
    pub fn jetstream(jetstream: jetstream::Context, prefix: impl Into<String>) -> Self {
        Self {
            backend: BundleAuditBackend::JetStream {
                jetstream,
                prefix: prefix.into(),
            },
        }
    }

    pub fn recording() -> (Self, Arc<Mutex<Vec<RecordedBundleAudit>>>) {
        let sink = Arc::new(Mutex::new(Vec::new()));
        let publisher = Self {
            backend: BundleAuditBackend::Recording(Arc::clone(&sink)),
        };
        (publisher, sink)
    }

    pub async fn emit(&self, event: &'static str, fields: BundleAuditFields) {
        match &self.backend {
            BundleAuditBackend::JetStream { jetstream, prefix } => {
                let subject = audit::audit_publish_subject(prefix, "control", "internal", event);
                let envelope = envelope_for_event(event, &fields);
                audit::publish_audit(jetstream, subject, &envelope, Duration::from_secs(5)).await;
            }
            BundleAuditBackend::Recording(sink) => {
                sink.lock()
                    .await
                    .push(RecordedBundleAudit {
                        event,
                        fields,
                    });
            }
        }
    }

    pub async fn load_succeeded(
        &self,
        bundle_id: &str,
        revision: u64,
        digest_hex: &str,
        previous_digest: Option<&str>,
    ) {
        self.emit(
            EVENT_LOAD_SUCCEEDED,
            BundleAuditFields {
                bundle_id: bundle_id.to_string(),
                revision,
                error_class: None,
                policy_bundle_digest: Some(digest_hex.to_string()),
                previous_digest: previous_digest.map(str::to_string),
            },
        )
        .await;
    }

    pub async fn load_failed(
        &self,
        bundle_id: &str,
        revision: u64,
        error: &BundleLoadError,
    ) {
        self.emit(
            EVENT_LOAD_FAILED,
            BundleAuditFields {
                bundle_id: bundle_id.to_string(),
                revision,
                error_class: Some(error_class(error)),
                policy_bundle_digest: None,
                previous_digest: None,
            },
        )
        .await;
    }

    pub async fn hot_swapped(
        &self,
        bundle_id: &str,
        revision: u64,
        digest_hex: &str,
        previous_digest: &str,
    ) {
        self.emit(
            EVENT_HOT_SWAPPED,
            BundleAuditFields {
                bundle_id: bundle_id.to_string(),
                revision,
                error_class: None,
                policy_bundle_digest: Some(digest_hex.to_string()),
                previous_digest: Some(previous_digest.to_string()),
            },
        )
        .await;
    }

    pub async fn rolled_back(
        &self,
        bundle_id: &str,
        revision: u64,
        restored_digest: &str,
        superseded_digest: &str,
    ) {
        self.emit(
            EVENT_ROLLED_BACK,
            BundleAuditFields {
                bundle_id: bundle_id.to_string(),
                revision,
                error_class: None,
                policy_bundle_digest: Some(restored_digest.to_string()),
                previous_digest: Some(superseded_digest.to_string()),
            },
        )
        .await;
    }
}

fn envelope_for_event(event: &str, fields: &BundleAuditFields) -> AuditEnvelope {
    let control_subject = format!("mcp.control.bundle.{event}");
    AuditEnvelope::new(
        control_subject.clone(),
        control_subject,
        "control",
        "internal",
        event.to_string(),
        None,
        None,
        None,
        IdentitySource::Anonymous,
        Some(serde_json::to_value(fields).unwrap_or(serde_json::Value::Null)),
        None,
    )
}

pub fn error_class(error: &BundleLoadError) -> String {
    match error {
        BundleLoadError::SignatureMissing
        | BundleLoadError::SignatureMalformed(_)
        | BundleLoadError::SignatureInvalid
        | BundleLoadError::SignatureVerificationUnavailable { .. }
        | BundleLoadError::UntrustedSigner { .. } => "signature".into(),
        BundleLoadError::ManifestMissing
        | BundleLoadError::ManifestParse(_)
        | BundleLoadError::ManifestInvalid(_)
        | BundleLoadError::DeprecatedManifestFilename { .. } => "manifest".into(),
        BundleLoadError::ContentHashMismatch { .. }
        | BundleLoadError::UnknownMember { .. }
        | BundleLoadError::MemberMissing { .. }
        | BundleLoadError::MemberTooLarge { .. }
        | BundleLoadError::Archive(_)
        | BundleLoadError::ArchiveTooLarge { .. } => "archive".into(),
        BundleLoadError::UnsupportedTargetWit { .. } | BundleLoadError::GatewayTooOld { .. } => {
            "compatibility".into()
        }
        BundleLoadError::KvEmpty { .. }
        | BundleLoadError::KvFetch(_)
        | BundleLoadError::KvWatch(_)
        | BundleLoadError::RevisionNotFound { .. }
        | BundleLoadError::BundleNotLoaded => "kv".into(),
    }
}
