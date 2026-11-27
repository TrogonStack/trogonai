use crate::ResourcePoolIdError;

/// A non-nested Proxmox resource pool id.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize)]
#[serde(transparent)]
pub struct ResourcePoolId(String);

impl std::str::FromStr for ResourcePoolId {
    type Err = ResourcePoolIdError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let valid = !value.is_empty()
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'));
        if valid {
            Ok(Self(value.to_owned()))
        } else {
            Err(ResourcePoolIdError)
        }
    }
}

impl std::fmt::Display for ResourcePoolId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

#[cfg(test)]
mod tests;
