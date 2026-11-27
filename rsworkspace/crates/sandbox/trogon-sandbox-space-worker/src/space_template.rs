use trogon_sandbox_contract::{OciImageRef, SandboxImage};

use crate::SpaceTemplateError;

/// The SPACE template every lease of a worker is created from, such as `seed:latest`.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct SpaceTemplate {
    text: String,
    reference: String,
}

impl SpaceTemplate {
    /// Parses a non-empty template name without spaces or NUL bytes.
    pub fn parse(value: impl Into<String>) -> Result<Self, SpaceTemplateError> {
        let text = value.into();
        if text.is_empty() {
            return Err(SpaceTemplateError::Empty);
        }
        if text.contains(['\0', ' ']) {
            return Err(SpaceTemplateError::Character);
        }
        let reference = OciImageRef::parse_unqualified(&text).allowlist_string();
        Ok(Self { text, reference })
    }

    /// The template name as SPACE spells it.
    pub fn as_str(&self) -> &str {
        &self.text
    }

    /// Whether a lease asking for `image` is asking for this template.
    pub fn serves(&self, image: &SandboxImage) -> bool {
        matches!(
            image,
            SandboxImage::Registry { reference, .. }
                if reference.allowlist_string() == self.reference
        )
    }
}
