use trogon_peer::{PeerValueError, SealedContentRef, TenantId, TrustedContent};
#[derive(Clone, PartialEq, Eq)]
pub struct ContentBinding {
    tenant: TenantId,
    content: TrustedContent,
}
impl ContentBinding {
    pub fn from_content(tenant: TenantId, content: TrustedContent) -> Self {
        Self { tenant, content }
    }
    pub fn from_port(
        tenant: TenantId,
        reference: SealedContentRef,
        size: u32,
        semantic_fingerprint: Vec<u8>,
    ) -> Result<Self, PeerValueError> {
        let content = TrustedContent::from_sealed_port(reference, size, semantic_fingerprint)?;
        Ok(Self::from_content(tenant, content))
    }
    pub fn tenant(&self) -> &TenantId {
        &self.tenant
    }
    pub fn content(&self) -> &TrustedContent {
        &self.content
    }
}
