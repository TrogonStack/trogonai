use trogon_policy_eval::{Action, Narrowing, RequestRuleEngine};

use crate::SessionPolicy;

/// Compose with platform permits and live holds. Passing this ceiling grants nothing.
impl RequestRuleEngine for SessionPolicy {
    type Facts = Action;

    fn narrow(&self, facts: &Action) -> Narrowing {
        let admitted = match facts {
            Action::EgressConnect { host, port } => self.permits_privacy_egress(host.as_str(), port.get()),
            Action::ModelUse { model } => self.permits_model(model.as_str()),
            // Delegation needs child policy and private context admission of its own.
            _ => false,
        };
        if admitted {
            Narrowing::Pass
        } else {
            Narrowing::Deny(self.rule_name().clone())
        }
    }
}
