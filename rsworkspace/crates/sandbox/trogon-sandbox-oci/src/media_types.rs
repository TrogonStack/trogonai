use crate::constants::ARTIFACT_TYPES;
use trogon_sandbox_contract::ArtifactType;

pub(crate) fn artifact_type_from_media_type(media_type: &str) -> Option<ArtifactType> {
    ARTIFACT_TYPES
        .into_iter()
        .find(|artifact_type| artifact_type.media_type() == media_type)
}

pub(crate) fn manifest_artifact_type(artifact_type: Option<&str>, config_media_type: &str) -> Option<ArtifactType> {
    artifact_type
        .and_then(artifact_type_from_media_type)
        .or_else(|| artifact_type_from_media_type(config_media_type))
}
