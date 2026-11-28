use trogon_sandbox_contract::{CredentialId, CredentialProvider, TenantId};

/// What a sealed credential payload is bound to, so a record copied under another provider, tenant, credential
/// id or version fails to open.
pub(crate) struct RecordBinding<'a> {
    pub(crate) provider: CredentialProvider,
    pub(crate) tenant: &'a TenantId,
    pub(crate) credential_id: &'a CredentialId,
    pub(crate) version: u32,
}

pub(crate) fn push_field(out: &mut Vec<u8>, field: &[u8]) {
    out.extend_from_slice(&(field.len() as u32).to_be_bytes());
    out.extend_from_slice(field);
}

impl RecordBinding<'_> {
    /// `provider || tenant || credential_id || version`, each field length-prefixed so field boundaries cannot be
    /// shifted, under a domain label.
    pub(crate) fn data_aad(&self) -> Vec<u8> {
        let mut out = Vec::new();
        push_field(&mut out, b"sandbox.credential.data.v2");
        push_field(&mut out, self.provider.as_str().as_bytes());
        push_field(&mut out, self.tenant.as_str().as_bytes());
        push_field(&mut out, self.credential_id.as_str().as_bytes());
        out.extend_from_slice(&self.version.to_be_bytes());
        out
    }
}

#[cfg(test)]
mod tests;
