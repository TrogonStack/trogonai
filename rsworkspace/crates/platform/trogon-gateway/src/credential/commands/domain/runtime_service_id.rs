use std::collections::BTreeSet;
use std::fmt;
use std::sync::Arc;

const MAX_RUNTIME_SERVICE_ID_LEN: usize = 128;

/// Identity of a runtime service permitted to resolve a credential.
///
/// Deliberately narrower than a free-form string: these values end up in NATS
/// subjects and audit facts, so the charset matches `SourceIntegrationId` and
/// excludes anything that would need escaping downstream.
#[derive(Clone, Eq, Ord, PartialEq, PartialOrd)]
pub struct RuntimeServiceId(Arc<str>);

impl RuntimeServiceId {
    pub fn new(value: impl AsRef<str>) -> Result<Self, RuntimeServiceIdError> {
        let value = value.as_ref();
        if value.is_empty() {
            return Err(RuntimeServiceIdError::Empty);
        }
        let char_count = value.chars().count();
        if char_count > MAX_RUNTIME_SERVICE_ID_LEN {
            return Err(RuntimeServiceIdError::TooLong(char_count));
        }
        for ch in value.chars() {
            if !is_runtime_service_id_char(ch) {
                return Err(RuntimeServiceIdError::InvalidCharacter(ch));
            }
        }
        Ok(Self(Arc::from(value)))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for RuntimeServiceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("RuntimeServiceId").field(&self.as_str()).finish()
    }
}

impl fmt::Display for RuntimeServiceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The runtime-service restriction on a credential.
///
/// Mirrors `AllowedHosts`: `Unrestricted` and an empty `Only` set are different
/// states, and a restriction with no caller identity supplied denies.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum AllowedRuntimeServices {
    #[default]
    Unrestricted,
    Only(BTreeSet<RuntimeServiceId>),
}

impl AllowedRuntimeServices {
    pub fn only<I, T>(services: I) -> Result<Self, RuntimeServiceIdError>
    where
        I: IntoIterator<Item = T>,
        T: AsRef<str>,
    {
        let services = services
            .into_iter()
            .map(RuntimeServiceId::new)
            .collect::<Result<BTreeSet<_>, _>>()?;
        Ok(Self::Only(services))
    }

    pub fn is_unrestricted(&self) -> bool {
        matches!(self, Self::Unrestricted)
    }

    pub fn permits(&self, candidate: Option<&RuntimeServiceId>) -> bool {
        match self {
            Self::Unrestricted => true,
            Self::Only(services) => match candidate {
                Some(candidate) => services.contains(candidate),
                None => false,
            },
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum RuntimeServiceIdError {
    #[error("runtime service id must not be empty")]
    Empty,
    #[error("runtime service id exceeds maximum length: {0}")]
    TooLong(usize),
    #[error("runtime service id contains invalid character '{0}'")]
    InvalidCharacter(char),
}

fn is_runtime_service_id_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_subject_safe_ids() {
        assert_eq!(
            RuntimeServiceId::new("trogon-gateway").unwrap().as_str(),
            "trogon-gateway"
        );
        assert_eq!(RuntimeServiceId::new("worker_2").unwrap().as_str(), "worker_2");
    }

    #[test]
    fn rejects_subject_unsafe_ids() {
        assert_eq!(RuntimeServiceId::new(""), Err(RuntimeServiceIdError::Empty));
        assert_eq!(
            RuntimeServiceId::new("trogon.gateway"),
            Err(RuntimeServiceIdError::InvalidCharacter('.'))
        );
        assert_eq!(
            RuntimeServiceId::new("trogon/gateway"),
            Err(RuntimeServiceIdError::InvalidCharacter('/'))
        );
        assert_eq!(
            RuntimeServiceId::new("trogon gateway"),
            Err(RuntimeServiceIdError::InvalidCharacter(' '))
        );
        assert_eq!(
            RuntimeServiceId::new("*"),
            Err(RuntimeServiceIdError::InvalidCharacter('*'))
        );
    }

    #[test]
    fn unrestricted_permits_an_absent_identity() {
        assert!(AllowedRuntimeServices::Unrestricted.permits(None));
    }

    #[test]
    fn restricted_denies_an_absent_identity() {
        let services = AllowedRuntimeServices::only(["trogon-gateway"]).unwrap();
        let allowed = RuntimeServiceId::new("trogon-gateway").unwrap();
        let other = RuntimeServiceId::new("some-other-worker").unwrap();

        assert!(services.permits(Some(&allowed)));
        assert!(!services.permits(Some(&other)));
        assert!(!services.permits(None));
    }

    #[test]
    fn empty_allow_list_denies_everything() {
        let services = AllowedRuntimeServices::Only(BTreeSet::new());
        let any = RuntimeServiceId::new("trogon-gateway").unwrap();

        assert!(!services.permits(Some(&any)));
        assert!(!services.permits(None));
    }
}
