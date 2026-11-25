use std::fmt::{self, Write as _};

use trogon_audit::AuditName;
use trogon_nats::NatsToken;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopedSnapshotKey(String);

impl ScopedSnapshotKey {
    pub fn new(tenant: &AuditName, project: &NatsToken) -> Self {
        Self(format!("t{}.p{}", encoded(tenant.as_str()), encoded(project.as_str())))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ScopedSnapshotKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

fn encoded(value: &str) -> String {
    let mut result = String::with_capacity(value.len() * 2);
    for byte in value.bytes() {
        let _ = write!(result, "{byte:02x}");
    }
    result
}
