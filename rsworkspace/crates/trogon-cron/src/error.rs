#[derive(Debug)]
pub enum CronError {
    Kv(String),
    Publish(String),
    Serde(serde_json::Error),
    InvalidCronExpression { expr: String, reason: String },
    InvalidJobConfig { reason: String },
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
            Self::InvalidJobConfig { reason } => write!(f, "Invalid job config: {reason}"),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kv_display() {
        let e = CronError::Kv("bucket gone".to_string());
        assert_eq!(e.to_string(), "KV error: bucket gone");
    }

    #[test]
    fn publish_display() {
        let e = CronError::Publish("conn lost".to_string());
        assert_eq!(e.to_string(), "Publish error: conn lost");
    }

    #[test]
    fn invalid_cron_expression_display() {
        let e = CronError::InvalidCronExpression {
            expr: "bad-expr".to_string(),
            reason: "5 fields only".to_string(),
        };
        let s = e.to_string();
        assert!(s.contains("bad-expr"), "got: {s}");
        assert!(s.contains("5 fields only"), "got: {s}");
    }

    #[test]
    fn invalid_job_config_display() {
        let e = CronError::InvalidJobConfig { reason: "bin not absolute".to_string() };
        let s = e.to_string();
        assert!(s.contains("Invalid job config"), "got: {s}");
        assert!(s.contains("bin not absolute"), "got: {s}");
    }

    #[test]
    fn serde_error_display_and_source() {
        let raw = serde_json::from_str::<i32>("{bad}").unwrap_err();
        let e: CronError = raw.into();
        assert!(e.to_string().contains("Serialization error"), "got: {e}");
        assert!(std::error::Error::source(&e).is_some(), "Serde must have a source");
    }

    #[test]
    fn io_error_display_and_source() {
        let raw = std::io::Error::new(std::io::ErrorKind::NotFound, "no such file");
        let e: CronError = raw.into();
        assert!(e.to_string().contains("IO error"), "got: {e}");
        assert!(std::error::Error::source(&e).is_some(), "Io must have a source");
    }

    #[test]
    fn kv_and_publish_errors_have_no_source() {
        assert!(std::error::Error::source(&CronError::Kv("x".to_string())).is_none());
        assert!(std::error::Error::source(&CronError::Publish("x".to_string())).is_none());
    }

    #[test]
    fn from_serde_json_error_wraps_as_serde_variant() {
        let raw = serde_json::from_str::<i32>("\"not a number\"").unwrap_err();
        let e: CronError = raw.into();
        assert!(matches!(e, CronError::Serde(_)));
    }

    #[test]
    fn from_io_error_wraps_as_io_variant() {
        let raw = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied");
        let e: CronError = raw.into();
        assert!(matches!(e, CronError::Io(_)));
    }
}
