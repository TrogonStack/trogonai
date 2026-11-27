use std::fmt::{self, Write as _};

use sha2::{Digest, Sha256};
use trogon_sandbox_contract::LeaseId;

use super::constants::{ANONYMOUS_PREFIX, NAMED_DIGEST_HEX_LEN, NAMED_PREFIX};
use super::dns_label::validate_dns1123_label;
use super::managed::is_managed;
use super::{Atespace, NamingError};

/// A Substrate actor name inside an [`Atespace`].
#[derive(Debug, Clone, Eq, PartialEq, Hash, PartialOrd, Ord)]
pub struct ActorName(String);

impl ActorName {
    /// The actor backing one anonymous lease. Reversible: see [`ActorName::lease_id`].
    pub fn for_lease(lease_id: &LeaseId) -> Result<Self, NamingError> {
        let lowered = lease_id.as_str().to_ascii_lowercase();
        if lowered.is_empty() {
            return Err(NamingError::LeaseIdNotALabel {
                lease_id: lease_id.as_str().to_string(),
                reason: "empty".to_string(),
            });
        }
        if !lowered.chars().all(|c| c.is_ascii_alphanumeric()) {
            return Err(NamingError::LeaseIdNotALabel {
                lease_id: lease_id.as_str().to_string(),
                reason: "expected an alphanumeric identifier".to_string(),
            });
        }
        let name = format!("{ANONYMOUS_PREFIX}{lowered}");
        validate_dns1123_label(&name).map_err(|reason| NamingError::LeaseIdNotALabel {
            lease_id: lease_id.as_str().to_string(),
            reason: reason.to_string(),
        })?;
        Ok(Self(name))
    }

    /// The actor backing a named, persistent lease.
    ///
    /// Stable across lease ids on purpose. The broker mints a fresh `LeaseId` every time a
    /// named lease is re-established, so keying the actor by the lease id would strand the
    /// suspended actor holding the tenant's state. Keying it by the name is what makes "the
    /// suspended actor *is* the persisted lease state" true.
    ///
    /// Digested rather than embedded because a lease name is arbitrary tenant text and a
    /// DNS-1123 label is not.
    pub fn for_named_lease(tenant: &Atespace, name: &str) -> Self {
        let mut hasher = Sha256::new();
        // Length-prefixed so ("ab", "c") and ("a", "bc") cannot collide.
        hasher.update((tenant.as_str().len() as u64).to_be_bytes());
        hasher.update(tenant.as_str().as_bytes());
        hasher.update((name.len() as u64).to_be_bytes());
        hasher.update(name.as_bytes());
        let digest = hex(&hasher.finalize());
        Self(format!("{NAMED_PREFIX}{}", &digest[..NAMED_DIGEST_HEX_LEN]))
    }

    /// Adopt a name observed on the Substrate side, e.g. from `ListActors`.
    pub fn from_existing(name: &str) -> Option<Self> {
        is_managed(name).then(|| Self(name.to_string()))
    }

    /// The lease id this name was built from, when it was built from one.
    ///
    /// `None` for a named actor, whose lease id is not recoverable from the name by
    /// construction. Callers that need it consult the worker's actor index instead.
    pub fn lease_id(&self) -> Option<LeaseId> {
        self.0
            .strip_prefix(ANONYMOUS_PREFIX)
            .map(|rest| LeaseId::new(rest.to_ascii_uppercase()))
    }

    pub fn is_named_lease(&self) -> bool {
        self.0.starts_with(NAMED_PREFIX)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ActorName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut acc, b| {
        let _ = write!(acc, "{b:02x}");
        acc
    })
}
