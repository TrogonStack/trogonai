use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemNoticeType {
    AppRateLimited,
}

impl SystemNoticeType {
    pub fn from_payload_type(payload_type: &str) -> Option<Self> {
        match payload_type {
            "app_rate_limited" => Some(Self::AppRateLimited),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::AppRateLimited => "app_rate_limited",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemMessageId(String);

impl SystemMessageId {
    pub fn new(body: &[u8]) -> Self {
        Self(format!("system-{}", hex::encode(Sha256::digest(body))))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests;
