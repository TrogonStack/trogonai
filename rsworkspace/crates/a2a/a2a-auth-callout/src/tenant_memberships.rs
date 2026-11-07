use crate::admission_keys::TenantAccount;
use crate::credentials::oidc::OidcIssuerUrl;
use crate::jwt::ExternalSubject;
use crate::tenant_admission::{
    TenantAdmission, TenantAdmissionDecision, TenantAdmissionError, TenantAdmissionRequest, TenantMembership,
};
use serde::Deserialize;
use std::collections::BTreeSet;
use trogon_identity_types::SubjectPrincipal;

use crate::constants::{MAX_MEMBERSHIPS, MAX_REGISTRY_BYTES};

#[derive(Debug, Clone)]
pub struct TenantMemberships(BTreeSet<TenantMembership>);

impl TenantMemberships {
    pub fn new(entries: impl IntoIterator<Item = TenantMembership>) -> Result<Self, TenantAdmissionError> {
        let mut members = BTreeSet::new();
        for entry in entries {
            if !members.insert(entry) {
                return Err(TenantAdmissionError::Duplicate);
            }
            if members.len() > MAX_MEMBERSHIPS {
                return Err(TenantAdmissionError::TooManyMemberships);
            }
        }
        Ok(Self(members))
    }

    pub fn from_json(bytes: &[u8]) -> Result<Self, TenantAdmissionError> {
        if bytes.len() > MAX_REGISTRY_BYTES {
            return Err(TenantAdmissionError::TooLarge);
        }
        let document: MembershipDocumentWire = serde_json::from_slice(bytes).map_err(TenantAdmissionError::Json)?;
        if document.version != 1 {
            return Err(TenantAdmissionError::UnsupportedVersion);
        }
        if document.memberships.len() > MAX_MEMBERSHIPS {
            return Err(TenantAdmissionError::TooManyMemberships);
        }
        let entries = document
            .memberships
            .into_iter()
            .map(|member| {
                let tenant = TenantAccount::parse(&member.account).map_err(TenantAdmissionError::Account)?;
                match member.identity {
                    MembershipIdentityWire::Oidc { issuer, subject } => {
                        let issuer = OidcIssuerUrl::parse(issuer).map_err(TenantAdmissionError::OidcIssuer)?;
                        let subject = ExternalSubject::new(subject).map_err(TenantAdmissionError::Claims)?;
                        TenantMembership::for_oidc_subject(tenant, &issuer, &subject)
                    }
                    MembershipIdentityWire::LegacyMtls { subject } => {
                        let subject = ExternalSubject::new(subject).map_err(TenantAdmissionError::Claims)?;
                        TenantMembership::for_legacy_mtls_subject(tenant, &subject)
                    }
                    MembershipIdentityWire::LegacyApiKey { subject } => {
                        let subject = ExternalSubject::new(subject).map_err(TenantAdmissionError::Claims)?;
                        TenantMembership::for_legacy_api_key_subject(tenant, &subject)
                    }
                    MembershipIdentityWire::Principal {
                        principal_kind,
                        principal_id,
                    } => {
                        let principal = SubjectPrincipal::from_tokens(&principal_kind, &principal_id)
                            .map_err(TenantAdmissionError::Principal)?;
                        Ok(TenantMembership::new(tenant, principal))
                    }
                }
            })
            .collect::<Result<Vec<_>, TenantAdmissionError>>()?;
        Self::new(entries)
    }

    pub(crate) fn decision(&self, request: TenantAdmissionRequest<'_>) -> TenantAdmissionDecision {
        let member = TenantMembership::new(request.tenant().clone(), request.principal().clone());
        if self.0.contains(&member) {
            TenantAdmissionDecision::Permit
        } else {
            TenantAdmissionDecision::Deny
        }
    }
}

#[async_trait::async_trait]
impl TenantAdmission for TenantMemberships {
    async fn authorize(
        &self,
        request: TenantAdmissionRequest<'_>,
    ) -> Result<TenantAdmissionDecision, TenantAdmissionError> {
        Ok(self.decision(request))
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MembershipDocumentWire {
    version: u32,
    memberships: Vec<MembershipEntryWire>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MembershipEntryWire {
    account: String,
    identity: MembershipIdentityWire,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum MembershipIdentityWire {
    Oidc {
        issuer: String,
        subject: String,
    },
    LegacyMtls {
        subject: String,
    },
    LegacyApiKey {
        subject: String,
    },
    Principal {
        principal_kind: String,
        principal_id: String,
    },
}
