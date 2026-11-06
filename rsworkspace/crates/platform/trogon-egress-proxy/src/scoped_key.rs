use std::fmt::Write as _;

use trogon_audit::AuditName;
use trogon_nats::NatsToken;

pub(crate) fn scoped_key(tenant: &AuditName, project: &NatsToken) -> String {
    format!("t{}.p{}", encoded(tenant.as_str()), encoded(project.as_str()))
}

fn encoded(value: &str) -> String {
    let mut result = String::with_capacity(value.len() * 2);
    for byte in value.bytes() {
        let _ = write!(result, "{byte:02x}");
    }
    result
}
