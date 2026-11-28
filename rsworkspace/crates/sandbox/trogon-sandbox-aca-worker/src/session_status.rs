use serde::Deserialize;

/// Whether a session can take work yet.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum SessionStatus {
    Ready,
    /// Any status that is not ready, as the service reported it.
    Pending(String),
}

/// Why a session description does not say whether the session is ready.
#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
pub enum SessionStatusError {
    #[error("the session description is empty")]
    EmptyBody,
    #[error("the session description is not JSON: {0}")]
    NotJson(String),
    #[error("the session description names no status")]
    NoStatus,
}

#[derive(Deserialize)]
struct SessionBody {
    properties: Option<SessionProperties>,
    status: Option<String>,
}

#[derive(Deserialize)]
struct SessionProperties {
    status: Option<String>,
}

impl SessionStatus {
    /// Classifies a status the service reported, ignoring case.
    pub fn parse(raw: &str) -> Self {
        match raw.to_ascii_lowercase().as_str() {
            "ready" | "running" | "active" | "allocated" | "succeeded" => Self::Ready,
            _ => Self::Pending(raw.to_owned()),
        }
    }

    /// The status a session description carries: `properties.status`, then `status`. An empty body,
    /// a body that is not JSON, and a body that names no status are errors: none of them says the session
    /// is ready, so none may be taken as ready.
    pub fn from_body(body: &str) -> Result<Self, SessionStatusError> {
        if body.trim().is_empty() {
            return Err(SessionStatusError::EmptyBody);
        }
        let parsed: SessionBody =
            serde_json::from_str(body).map_err(|error| SessionStatusError::NotJson(error.to_string()))?;
        parsed
            .properties
            .and_then(|properties| properties.status)
            .or(parsed.status)
            .map(|status| Self::parse(&status))
            .ok_or(SessionStatusError::NoStatus)
    }

    pub fn is_ready(&self) -> bool {
        matches!(self, Self::Ready)
    }
}
