use crate::admission_keys::TenantAccount;
use crate::credentials::oidc::OidcIssuerUrl;
use crate::jwt::{AccountName, CallerNamespace, ExternalSubject, derive_caller_id};
use crate::tenant_admission_error::TenantAdmissionError;
use trogon_identity_types::SubjectPrincipal;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct TenantMembership {
    tenant: TenantAccount,
    principal: SubjectPrincipal,
}

impl TenantMembership {
    pub fn new(tenant: TenantAccount, principal: SubjectPrincipal) -> Self {
        Self { tenant, principal }
    }

    pub fn from_verified_credential(
        tenant: TenantAccount,
        credential: &crate::VerifiedCredential,
    ) -> Result<Self, TenantAdmissionError> {
        let claims = credential.claims();
        if tenant.as_str() != claims.aud.as_str() {
            return Err(TenantAdmissionError::AudienceMismatch);
        }
        let principal = claims.subject_principal().map_err(TenantAdmissionError::Claims)?;
        Ok(Self::new(tenant, principal))
    }

    pub fn for_oidc_subject(
        tenant: TenantAccount,
        issuer: &OidcIssuerUrl,
        subject: &ExternalSubject,
    ) -> Result<Self, TenantAdmissionError> {
        Self::for_source(tenant, CallerNamespace::Oidc { issuer }, subject)
    }

    pub fn for_legacy_mtls_subject(
        tenant: TenantAccount,
        subject: &ExternalSubject,
    ) -> Result<Self, TenantAdmissionError> {
        Self::for_source(tenant, CallerNamespace::Mtls, subject)
    }

    pub fn for_legacy_api_key_subject(
        tenant: TenantAccount,
        subject: &ExternalSubject,
    ) -> Result<Self, TenantAdmissionError> {
        Self::for_source(tenant, CallerNamespace::ApiKey, subject)
    }

    fn for_source(
        tenant: TenantAccount,
        source: CallerNamespace<'_>,
        subject: &ExternalSubject,
    ) -> Result<Self, TenantAdmissionError> {
        let caller = derive_caller_id(source, subject.as_str(), &AccountName::new(tenant.as_str()))
            .map_err(TenantAdmissionError::Claims)?;
        let id = trogon_identity_types::PrincipalId::new(caller.as_str()).map_err(TenantAdmissionError::PrincipalId)?;
        let principal =
            SubjectPrincipal::from_credential(source.principal_kind(), &id).map_err(TenantAdmissionError::Principal)?;
        Ok(Self::new(tenant, principal))
    }

    pub fn tenant(&self) -> &TenantAccount {
        &self.tenant
    }

    pub fn principal(&self) -> &SubjectPrincipal {
        &self.principal
    }
}
