use std::str::FromStr;

use crate::WorkerIdentityError;

/// The stable identity of one worker replica, such as its StatefulSet pod name.
///
/// It is stamped on every resource the worker creates, so it is a Kubernetes label value: at most
/// 63 characters, alphanumeric at both ends, with `-`, `_` and `.` allowed in between.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct WorkerIdentity(String);

impl WorkerIdentity {
    const MAX_LENGTH: usize = 63;

    /// A fresh identity, for a replica that cannot recover resources from a previous process.
    pub fn generate() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for WorkerIdentity {
    type Err = WorkerIdentityError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let valid = !value.is_empty()
            && value.len() <= Self::MAX_LENGTH
            && value
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
            && value.starts_with(|c: char| c.is_ascii_alphanumeric())
            && value.ends_with(|c: char| c.is_ascii_alphanumeric());
        if valid {
            Ok(Self(value.to_owned()))
        } else {
            Err(WorkerIdentityError(value.to_owned()))
        }
    }
}

#[cfg(test)]
mod tests;
