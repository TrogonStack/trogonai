//! §1889 Artifact browser.
//!
//! Lists a session's artifacts over the specified `ArtifactMetadata` contract (§937-953:
//! `artifact_id`, `sha256`, `size_bytes`, `mime`, `preview`, `storage_ref`, `truncated`,
//! `availability`, retention, …). A browser is a pure, deterministic read: it surfaces each
//! artifact's metadata (never the raw bytes — those live in the object store / claim-check)
//! plus a summary, so a UI or CLI can browse what a session produced. No I/O.

use trogonai_session_contracts::{ArtifactMetadata, ArtifactSourceAvailability};

/// One browsable artifact — metadata only (§937), never raw bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactBrowseEntry {
    pub artifact_id: String,
    pub mime: String,
    pub size_bytes: u64,
    pub sha256: String,
    pub preview: String,
    pub truncated: bool,
    /// True when the content was claim-checked to the object store (has a `storage_ref`),
    /// false when it is small/inline (§925-927 inline vs object-store).
    pub claim_checked: bool,
    pub availability: &'static str,
    pub source_url: Option<String>,
}

/// Summary + per-artifact listing for a session (§1889).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ArtifactBrowse {
    pub total: usize,
    pub total_bytes: u64,
    pub truncated_count: usize,
    /// Count of artifacts that are not retrievable (`ExternalRef`/unavailable) — surfaced so
    /// a browser shows degraded artifacts explicitly (§ Error UX `artifact_unavailable`).
    pub unavailable_count: usize,
    pub entries: Vec<ArtifactBrowseEntry>,
}

/// Stable label for an artifact's source availability (§983 `availability`).
pub fn availability_label(availability: Option<ArtifactSourceAvailability>) -> &'static str {
    match availability {
        Some(ArtifactSourceAvailability::Stored) => "stored",
        Some(ArtifactSourceAvailability::ExternalRef) => "external_ref",
        Some(ArtifactSourceAvailability::Unspecified) | None => "unspecified",
    }
}

/// Browse a session's artifacts (§1889). Pure + deterministic (preserves input order).
pub fn browse_artifacts(artifacts: &[ArtifactMetadata]) -> ArtifactBrowse {
    let mut browse = ArtifactBrowse {
        total: artifacts.len(),
        ..ArtifactBrowse::default()
    };
    for artifact in artifacts {
        browse.total_bytes += artifact.size_bytes;
        if artifact.truncated {
            browse.truncated_count += 1;
        }
        let availability = artifact.availability.as_known();
        if !matches!(availability, Some(ArtifactSourceAvailability::Stored)) {
            browse.unavailable_count += 1;
        }
        browse.entries.push(ArtifactBrowseEntry {
            artifact_id: artifact.artifact_id.clone(),
            mime: artifact.mime.clone(),
            size_bytes: artifact.size_bytes,
            sha256: artifact.sha256.clone(),
            preview: artifact.preview.clone(),
            truncated: artifact.truncated,
            claim_checked: !artifact.storage_ref.is_empty(),
            availability: availability_label(availability),
            source_url: artifact.source_url.clone(),
        });
    }
    browse
}

#[cfg(test)]
mod tests {
    use super::*;
    use buffa::EnumValue;

    fn artifact(id: &str, size: u64, mime: &str, storage_ref: &str, truncated: bool, availability: ArtifactSourceAvailability) -> ArtifactMetadata {
        ArtifactMetadata {
            artifact_id: id.to_string(),
            sha256: format!("sha_{id}"),
            size_bytes: size,
            mime: mime.to_string(),
            preview: format!("preview of {id}"),
            storage_ref: storage_ref.to_string(),
            truncated,
            availability: EnumValue::Known(availability),
            ..ArtifactMetadata::default()
        }
    }

    #[test]
    fn browse_summarizes_and_lists_artifacts() {
        let artifacts = vec![
            artifact("a1", 100, "text/plain", "", false, ArtifactSourceAvailability::Stored),
            artifact("a2", 5000, "image/png", "obj://b/sessions/s/a2", true, ArtifactSourceAvailability::Stored),
            artifact("a3", 0, "image/png", "", false, ArtifactSourceAvailability::ExternalRef),
        ];
        let browse = browse_artifacts(&artifacts);
        assert_eq!(browse.total, 3);
        assert_eq!(browse.total_bytes, 5100);
        assert_eq!(browse.truncated_count, 1);
        assert_eq!(browse.unavailable_count, 1); // a3 external_ref

        let a1 = &browse.entries[0];
        assert!(!a1.claim_checked, "inline artifact (no storage_ref) is not claim-checked");
        let a2 = &browse.entries[1];
        assert!(a2.claim_checked, "object-store artifact is claim-checked");
        assert_eq!(a2.availability, "stored");
        assert!(a2.preview.contains("a2"));
        let a3 = &browse.entries[2];
        assert_eq!(a3.availability, "external_ref");
    }

    #[test]
    fn empty_session_browses_to_zero() {
        let browse = browse_artifacts(&[]);
        assert_eq!(browse.total, 0);
        assert!(browse.entries.is_empty());
    }
}
