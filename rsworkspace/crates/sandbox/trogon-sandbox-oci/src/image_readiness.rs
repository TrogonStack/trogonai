/// Whether an image can run on the sandbox platform, from its registry manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageReadiness {
    Ready,
    /// Registry lookup failed in a way that may resolve on its own (image still
    /// propagating, transient network failure). Worth retrying later.
    Preparing {
        reason: String,
    },
    /// The manifest resolved but carries no linux/amd64 variant. Retrying will
    /// not help; the image itself needs to change.
    Incompatible {
        reason: String,
    },
}
