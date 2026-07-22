//! Provider introspection subject `trogon.proxy.providers` (S1).

use buffa::Message as _;
use bytes::Bytes;
use futures_util::StreamExt;
use std::collections::BTreeSet;
use std::time::{SystemTime, UNIX_EPOCH};

use trogonai_catalog_proto::ProviderSnapshot;

use crate::traits::NatsClient;

/// NATS request-reply subject for callable provider introspection.
pub fn providers_subject(prefix: &str) -> String {
    format!("{prefix}.proxy.providers")
}

/// Subscribe and serve provider snapshots until the NATS client closes.
pub async fn run_providers_listener<N: NatsClient>(
    nats: N,
    prefix: &str,
    known_providers: &[&str],
) -> Result<(), ProvidersError> {
    let subject = providers_subject(prefix);
    let mut sub = nats
        .subscribe(subject.clone())
        .await
        .map_err(|e| ProvidersError::Subscribe {
            subject: subject.clone(),
            source: e.to_string(),
        })?;

    tracing::info!(subject = %subject, "provider introspection listening");

    while let Some(msg) = sub.next().await {
        let Some(reply) = msg.reply else {
            continue;
        };
        let providers: Vec<String> = known_providers.iter().map(|s| (*s).to_string()).collect();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        let proto = ProviderSnapshot {
            providers,
            fetched_at_unix_secs: now,
            __buffa_unknown_fields: Default::default(),
        };
        let _ = nats
            .publish(reply.to_string(), Bytes::from(proto.encode_to_vec()))
            .await;
    }
    Ok(())
}

/// Derive callable providers from vault token keys (`tok_{provider}_...`).
pub fn providers_from_token_keys(keys: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut set = BTreeSet::new();
    for key in keys {
        if let Some(provider) = key.strip_prefix("tok_").and_then(|s| s.split('_').next())
            && !provider.is_empty()
        {
            set.insert(provider.to_string());
        }
    }
    set.into_iter().collect()
}

#[derive(Debug)]
pub enum ProvidersError {
    Subscribe { subject: String, source: String },
}

impl std::fmt::Display for ProvidersError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Subscribe { subject, source } => {
                write!(f, "failed to subscribe to '{subject}': {source}")
            }
        }
    }
}

impl std::error::Error for ProvidersError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn providers_subject_format() {
        assert_eq!(providers_subject("trogon"), "trogon.proxy.providers");
    }

    #[test]
    fn providers_from_token_keys_extracts_unique_providers() {
        let keys = vec![
            "tok_anthropic_prod_abc".into(),
            "tok_xai_dev_xyz".into(),
            "tok_anthropic_staging_def".into(),
        ];
        let providers = providers_from_token_keys(keys);
        assert_eq!(providers, vec!["anthropic", "xai"]);
    }
}
