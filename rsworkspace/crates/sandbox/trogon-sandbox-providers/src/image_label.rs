use trogon_sandbox_contract::SandboxImage;

/// The reference a provider hands to its engine for `image`.
pub(crate) fn sandbox_image_label(image: &SandboxImage) -> String {
    match image {
        SandboxImage::Registry { reference, .. } => reference.allowlist_string(),
        SandboxImage::Snapshot { reference, .. } | SandboxImage::HyperlightModule { reference } => {
            reference.allowlist_string()
        }
        SandboxImage::SnapshotTag { tag, .. } => format!("tag:{}:{}", tag.tenant, tag.name),
        SandboxImage::NativeSnapshot { reference } => {
            format!("native:{:?}:{}", reference.provider, reference.id)
        }
    }
}
