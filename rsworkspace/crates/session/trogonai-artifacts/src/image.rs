//! URL image resolution (§ Politica para imagenes URL y Base64 — "Imagen por URL"):
//! `fetch -> validate -> hash -> store bytes as artifact`, keeping `source_url` as
//! metadata so the URL is never the canonical truth. If the URL cannot be fetched
//! (auth/timeout/size/policy/network), only a degraded `external_ref` is kept; a
//! downstream Safety Gate blocks/confirms when an indispensable image exists only as an
//! external_ref (§ "si la imagen es indispensable y solo existe como external_ref").
//!
//! HTTP lives behind the injectable [`ImageFetcher`] trait so this domain crate stays
//! transport-free (ADR-0002 / ADR-0003); the runner provides the reqwest-backed adapter.

use buffa_types::google::protobuf::Timestamp;
use bytes::Bytes;
use trogonai_session_contracts::{ArtifactSourceAvailability, EventId, SessionId};

use crate::store::{StoreArtifactRequest, sniff_image_mime};

/// Limits applied when fetching a remote image (§ "fetchear la URL con limites de
/// timeout, tamano, redirects y content type"). Conservative initial values, revisable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FetchLimits {
    pub timeout_secs: u64,
    pub max_size_bytes: usize,
    pub max_redirects: u8,
}

impl Default for FetchLimits {
    fn default() -> Self {
        Self {
            timeout_secs: 10,
            max_size_bytes: 10 * 1024 * 1024,
            max_redirects: 3,
        }
    }
}

/// Successfully fetched image bytes plus the transport-declared content type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FetchedImage {
    pub content_type: Option<String>,
    pub bytes: Bytes,
}

/// Why a remote image could not be fetched (§ "si no se puede fetchear por auth,
/// timeout, tamano, politica o red").
#[derive(Debug, thiserror::Error)]
pub enum ImageFetchError {
    #[error("image fetch timed out")]
    Timeout,
    #[error("image exceeds the configured size limit")]
    TooLarge,
    #[error("image fetch blocked by egress policy")]
    Policy,
    #[error("image fetch network error: {0}")]
    Network(String),
    #[error("image fetch returned unexpected status {0}")]
    BadStatus(u16),
}

/// Fetches remote image bytes with the given limits. Implemented by a runner-side
/// adapter (reqwest + egress policy); this crate stays HTTP-free.
pub trait ImageFetcher {
    fn fetch(
        &self,
        url: &str,
        limits: &FetchLimits,
    ) -> impl std::future::Future<Output = Result<FetchedImage, ImageFetchError>> + Send;
}

/// Outcome of resolving a URL image source.
pub enum UrlImageOutcome {
    /// Fetched, validated and ready to store as an own artifact (availability = Stored).
    /// `sha256`/`size_bytes` are computed over the fetched bytes by `ArtifactStore::store`.
    Stored(Box<StoreArtifactRequest>),
    /// Could not be fetched; only a degraded external reference is kept
    /// (availability = ExternalRef). The Safety Gate decides if this blocks a switch.
    Degraded { source_url: String, reason: String },
}

/// Resolve a URL image per § "Imagen por URL": fetch -> validate the real MIME -> build a
/// store request over the fetched bytes, or degrade to an `external_ref` when the fetch
/// fails. The MIME is sniffed from the bytes and validated against the declared
/// content type.
pub async fn resolve_url_image<F: ImageFetcher>(
    fetcher: &F,
    session_id: SessionId,
    event_id: EventId,
    url: &str,
    fetched_at: Timestamp,
    limits: &FetchLimits,
) -> UrlImageOutcome {
    match fetcher.fetch(url, limits).await {
        Ok(image) => {
            let declared = image.content_type.clone();
            // § "validar MIME real": prefer the sniffed MIME; fall back to declared.
            let decoded = sniff_image_mime(&image.bytes)
                .or_else(|| declared.clone())
                .unwrap_or_else(|| "application/octet-stream".to_string());
            let mut request = StoreArtifactRequest::new(session_id, event_id, decoded.clone(), image.bytes);
            request.source_url = Some(url.to_string());
            request.fetched_at = Some(fetched_at);
            request.declared_mime = declared;
            request.decoded_mime = Some(decoded);
            request.availability = ArtifactSourceAvailability::Stored;
            UrlImageOutcome::Stored(Box::new(request))
        }
        Err(err) => UrlImageOutcome::Degraded {
            source_url: url.to_string(),
            reason: err.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct OkFetcher {
        bytes: Bytes,
        content_type: Option<String>,
    }

    impl ImageFetcher for OkFetcher {
        async fn fetch(&self, _url: &str, _limits: &FetchLimits) -> Result<FetchedImage, ImageFetchError> {
            Ok(FetchedImage {
                content_type: self.content_type.clone(),
                bytes: self.bytes.clone(),
            })
        }
    }

    struct FailFetcher;

    impl ImageFetcher for FailFetcher {
        async fn fetch(&self, _url: &str, _limits: &FetchLimits) -> Result<FetchedImage, ImageFetchError> {
            Err(ImageFetchError::Timeout)
        }
    }

    fn ts() -> Timestamp {
        Timestamp {
            seconds: 1_700_000_000,
            nanos: 0,
            ..Timestamp::default()
        }
    }

    fn ids() -> (SessionId, EventId) {
        (SessionId::new("sess_url").unwrap(), EventId::new("evt_url").unwrap())
    }

    #[tokio::test]
    async fn fetched_url_image_becomes_stored_request_with_provenance() {
        let png = Bytes::from_static(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x01]);
        let fetcher = OkFetcher {
            bytes: png.clone(),
            content_type: Some("image/png".to_string()),
        };
        let (sid, eid) = ids();
        let outcome =
            resolve_url_image(&fetcher, sid, eid, "https://host/y.png", ts(), &FetchLimits::default()).await;

        match outcome {
            UrlImageOutcome::Stored(request) => {
                assert_eq!(request.content, png);
                assert_eq!(request.source_url.as_deref(), Some("https://host/y.png"));
                assert_eq!(request.decoded_mime.as_deref(), Some("image/png"));
                assert_eq!(request.availability, ArtifactSourceAvailability::Stored);
                assert!(request.fetched_at.is_some());
            }
            UrlImageOutcome::Degraded { .. } => panic!("expected Stored"),
        }
    }

    #[tokio::test]
    async fn mime_is_validated_from_bytes_over_a_wrong_declared_type() {
        let png = Bytes::from_static(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]);
        let fetcher = OkFetcher {
            bytes: png,
            content_type: Some("image/jpeg".to_string()), // wrong on purpose
        };
        let (sid, eid) = ids();
        let outcome = resolve_url_image(&fetcher, sid, eid, "https://host/y", ts(), &FetchLimits::default()).await;
        if let UrlImageOutcome::Stored(request) = outcome {
            assert_eq!(request.declared_mime.as_deref(), Some("image/jpeg"));
            assert_eq!(request.decoded_mime.as_deref(), Some("image/png"));
            assert_eq!(request.mime, "image/png");
        } else {
            panic!("expected Stored");
        }
    }

    #[tokio::test]
    async fn unfetchable_url_image_degrades_to_external_ref() {
        let (sid, eid) = ids();
        let outcome = resolve_url_image(&FailFetcher, sid, eid, "https://host/y.png", ts(), &FetchLimits::default()).await;
        match outcome {
            UrlImageOutcome::Degraded { source_url, .. } => {
                assert_eq!(source_url, "https://host/y.png");
            }
            UrlImageOutcome::Stored(_) => panic!("expected Degraded"),
        }
    }
}
