use serde::{Deserialize, Serialize};

use crate::wasi_egress_rule_wire::WasiEgressRuleWire;
use crate::{Port, WasiEgressRuleError};

/// One outbound-socket allow rule for a WASI guest.
///
/// The worker resolves `host`, which is never empty, to concrete addresses and matches the guest's connection
/// attempts against that set, restricted to `port`. That differs from a domain rule, which an egress proxy matches
/// against a hostname from outside the process. Construction and deserialization both enforce the rules.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(try_from = "WasiEgressRuleWire", into = "WasiEgressRuleWire")]
pub struct WasiEgressRule {
    host: String,
    port: Port,
}

impl WasiEgressRule {
    pub fn new(host: impl Into<String>, port: Port) -> Result<Self, WasiEgressRuleError> {
        let host = host.into();
        if host.is_empty() {
            return Err(WasiEgressRuleError::EmptyHost);
        }
        Ok(Self { host, port })
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    pub fn port(&self) -> Port {
        self.port
    }
}

impl TryFrom<WasiEgressRuleWire> for WasiEgressRule {
    type Error = WasiEgressRuleError;

    fn try_from(wire: WasiEgressRuleWire) -> Result<Self, Self::Error> {
        Self::new(wire.host, wire.port)
    }
}

impl From<WasiEgressRule> for WasiEgressRuleWire {
    fn from(rule: WasiEgressRule) -> Self {
        Self {
            host: rule.host,
            port: rule.port,
        }
    }
}
