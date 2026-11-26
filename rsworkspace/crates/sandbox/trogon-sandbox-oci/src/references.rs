use oci_client::Reference;
use trogon_sandbox_contract::{Digest, OciArtifactRef, OciImageRef};

use crate::{OciError, RegistryHost};

fn parse_reference(reference: String) -> Result<Reference, OciError> {
    reference
        .parse()
        .map_err(|error| OciError::InvalidReference(format!("{error}")))
}

pub(crate) fn tag_reference(target: &OciArtifactRef) -> Result<Reference, OciError> {
    subject_tag_reference(target, &artifact_staging_tag(&target.digest))
}

pub(crate) fn digest_reference(target: &OciArtifactRef) -> Result<Reference, OciError> {
    digest_reference_for(&target.registry, &target.repository, &target.digest)
}

pub(crate) fn digest_reference_for(registry: &str, repository: &str, digest: &Digest) -> Result<Reference, OciError> {
    RegistryHost::new(registry)?;
    parse_reference(format!("{registry}/{repository}@{digest}"))
}

pub(crate) fn subject_tag_reference(subject: &OciArtifactRef, tag: &str) -> Result<Reference, OciError> {
    RegistryHost::new(&subject.registry)?;
    parse_reference(format!("{}/{}:{}", subject.registry, subject.repository, tag))
}

pub(crate) fn image_reference(image: &OciImageRef) -> Result<Reference, OciError> {
    let reference = if let Some(digest) = &image.digest {
        format!("{}/{}@{}", image.registry, image.repository, digest)
    } else if let Some(tag) = &image.tag {
        format!("{}/{}:{}", image.registry, image.repository, tag)
    } else {
        format!("{}/{}:latest", image.registry, image.repository)
    };
    parse_reference(reference)
}

/// Application-owned staging tags leave OCI's digest-derived referrer index tags free.
pub(crate) fn artifact_staging_tag(digest: &Digest) -> String {
    format!("{STAGING_TAG_PREFIX}{}", digest.as_str().replace(':', "-"))
}

pub(crate) const STAGING_TAG_PREFIX: &str = "sandbox-artifact-";

pub(crate) fn referrers_tag(digest: &Digest) -> String {
    digest.as_str().replace(':', "-")
}
