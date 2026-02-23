#[derive(Debug)]
pub enum CronError {
    Kv(String),
    Publish(String),
    Serde(serde_json::Error),
    InvalidCronExpression { expr: String, reason: String },
    Io(std::io::Error),
}

impl std::fmt::Display for CronError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Kv(msg) => write!(f, "KV error: {msg}"),
            Self::Publish(msg) => write!(f, "Publish error: {msg}"),
            Self::Serde(e) => write!(f, "Serialization error: {e}"),
            Self::InvalidCronExpression { expr, reason } => {
                write!(f, "Invalid cron expression '{expr}': {reason}")
            }
            Self::Io(e) => write!(f, "IO error: {e}"),
        }
    }
}

impl std::error::Error for CronError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Serde(e) => Some(e),
            Self::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<serde_json::Error> for CronError {
    fn from(e: serde_json::Error) -> Self {
        Self::Serde(e)
    }
}

impl From<std::io::Error> for CronError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}
