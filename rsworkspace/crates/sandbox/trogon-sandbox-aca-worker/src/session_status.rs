use serde::Deserialize;

/// Whether a session can take work yet.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum SessionStatus {
    Ready,
    /// Any status that is not ready, as the service reported it.
    Pending(String),
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

    /// The status a session description carries: `properties.status`, then `status`. A body that
    /// names none, or is not JSON, describes a ready session.
    pub fn from_body(body: &str) -> Self {
        serde_json::from_str::<SessionBody>(body)
            .ok()
            .and_then(|parsed| {
                parsed
                    .properties
                    .and_then(|properties| properties.status)
                    .or(parsed.status)
            })
            .map_or(Self::Ready, |status| Self::parse(&status))
    }

    pub fn is_ready(&self) -> bool {
        matches!(self, Self::Ready)
    }
}
