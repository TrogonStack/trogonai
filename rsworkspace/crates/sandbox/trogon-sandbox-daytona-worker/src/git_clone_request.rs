use std::fmt;

use serde::Serialize;

/// Body of a toolbox `git/clone` call. The credentials travel in their own fields, never inside the URL.
#[derive(Clone, Serialize)]
pub struct GitCloneRequest {
    pub url: String,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
}

impl fmt::Debug for GitCloneRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GitCloneRequest")
            .field("url", &self.url)
            .field("path", &self.path)
            .field("commit_id", &self.commit_id)
            .field("username", &self.username)
            .field("password", &self.password.as_ref().map(|_| "[REDACTED]"))
            .finish()
    }
}

#[cfg(test)]
mod tests;
