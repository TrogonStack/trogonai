use oci_client::manifest::OciDescriptor;

use crate::OciError;

/// The descriptor size as an unsigned byte count.
pub(crate) fn descriptor_size(descriptor: &OciDescriptor) -> Result<u64, OciError> {
    u64::try_from(descriptor.size).map_err(|_| OciError::Layout("OCI descriptor size is negative".into()))
}

/// Whether the descriptor points at foreign URLs instead of registry blobs.
pub(crate) fn has_foreign_urls(descriptor: &OciDescriptor) -> bool {
    descriptor.urls.as_ref().is_some_and(|urls| !urls.is_empty())
}
