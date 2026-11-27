use trogon_sandbox_contract::{CloneAuth, LeaseError, SecretRef, WorkspaceSource};
use url::Url;

use crate::constants::{CLONE_TOKEN_USER, MAX_SHA_LEN};

/// A workspace checkout, parsed once from the lease's workspace so the create path never re-checks it.
#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) struct CloneTarget {
    pub(crate) url: String,
    pub(crate) commit: String,
    pub(crate) credentials: Option<(String, String)>,
}

fn invalid(reason: &str) -> LeaseError {
    LeaseError::InvalidSpec {
        reason: reason.to_owned(),
    }
}

impl CloneTarget {
    /// `None` for an empty workspace. A workspace Daytona cannot provide is refused here, before a sandbox exists.
    pub(crate) fn parse(workspace: &WorkspaceSource) -> Result<Option<Self>, LeaseError> {
        let (url, sha, auth) = match workspace {
            WorkspaceSource::Empty(_) => return Ok(None),
            WorkspaceSource::Clone { url, sha, auth, .. } => (url, sha, auth),
            WorkspaceSource::HostMount { .. } => {
                return Err(invalid("Daytona cannot bind a host path into a sandbox"));
            }
            WorkspaceSource::S3Volume { .. } => return Err(invalid("Daytona cannot mount an S3 volume")),
        };
        let parsed = Url::parse(url).map_err(|_| invalid("the clone URL is not a URL"))?;
        if parsed.scheme() != "https" || parsed.host_str().is_none() {
            return Err(invalid("the clone URL must be an https URL"));
        }
        if !parsed.username().is_empty() || parsed.password().is_some() {
            return Err(invalid(
                "the clone URL must not carry credentials; pass them as the clone auth",
            ));
        }
        if parsed.query().is_some() || parsed.fragment().is_some() {
            return Err(invalid("the clone URL must not carry a query or a fragment"));
        }
        let hex = sha.bytes().all(|byte| byte.is_ascii_hexdigit());
        if !hex || !(7..=MAX_SHA_LEN).contains(&sha.len()) {
            return Err(invalid("the clone sha must be 7 to 64 hexadecimal characters"));
        }
        let credentials = match auth {
            CloneAuth::None(_) => None,
            CloneAuth::BearerToken {
                token_ref: SecretRef::Literal { value },
            } if !value.is_empty() => Some((CLONE_TOKEN_USER.to_owned(), value.clone())),
            CloneAuth::BearerToken {
                token_ref: SecretRef::Literal { .. },
            } => return Err(invalid("the clone token is empty")),
            CloneAuth::BearerToken {
                token_ref: SecretRef::NatsKv { .. },
            } => {
                return Err(invalid(
                    "a token stored in NATS key-value cannot be resolved by this worker",
                ));
            }
            CloneAuth::Basic { .. } => return Err(invalid("basic clone auth is not supported")),
        };
        Ok(Some(Self {
            url: url.clone(),
            commit: sha.clone(),
            credentials,
        }))
    }
}

#[cfg(test)]
mod tests;
