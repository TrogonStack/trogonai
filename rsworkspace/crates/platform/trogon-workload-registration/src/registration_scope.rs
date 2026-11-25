use spiffe::TrustDomain;
use trogon_nats::NatsToken;

use crate::constants::{HEX_ALPHABET, MAX_TRUST_DOMAIN_BYTES};
use crate::{RegistrationError, ScopeName};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistrationScope {
    tenant: ScopeName,
    project: NatsToken,
    trust_domain: TrustDomain,
}

impl RegistrationScope {
    pub fn new(tenant: ScopeName, project: ScopeName, trust_domain: &str) -> Result<Self, RegistrationError> {
        let domain = TrustDomain::new(trust_domain).map_err(RegistrationError::TrustDomain)?;
        if domain.as_str() != trust_domain || trust_domain.len() > MAX_TRUST_DOMAIN_BYTES {
            return Err(RegistrationError::NoncanonicalDomain);
        }
        Ok(Self {
            tenant,
            project: NatsToken::new(project.as_str()).map_err(RegistrationError::Project)?,
            trust_domain: domain,
        })
    }

    pub fn tenant(&self) -> &ScopeName {
        &self.tenant
    }
    pub fn project(&self) -> &NatsToken {
        &self.project
    }
    pub fn trust_domain(&self) -> &TrustDomain {
        &self.trust_domain
    }

    pub fn key(&self) -> String {
        fn hex(value: &str) -> String {
            value
                .bytes()
                .flat_map(|byte| [HEX_ALPHABET[(byte >> 4) as usize], HEX_ALPHABET[(byte & 15) as usize]])
                .map(char::from)
                .collect()
        }
        format!("t{}.p{}", hex(self.tenant.as_str()), hex(self.project.as_str()))
    }
}
