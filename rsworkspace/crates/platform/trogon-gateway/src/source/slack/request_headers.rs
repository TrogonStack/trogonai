use async_nats::HeaderMap;
use serde_json::Value;

use super::ack_budget::AckPlan;
use super::constants::{NATS_HEADER_ACTION_ID, NATS_HEADER_BLOCK_ID, NATS_HEADER_CALLBACK_ID};
use super::header_text::{HeaderText, InvalidHeaderTextError};

#[derive(Debug, Default)]
pub struct RequestHeaders(Vec<(&'static str, HeaderText)>);

impl RequestHeaders {
    pub fn from_payload(plan: AckPlan, payload: &Value) -> Result<Self, InvalidHeaderTextError> {
        let sources: &[(&'static str, &str)] = match plan {
            AckPlan::Acknowledge => &[],
            AckPlan::AwaitReply => &[(NATS_HEADER_CALLBACK_ID, "/view/callback_id")],
            AckPlan::RequestOnly => &[
                (NATS_HEADER_ACTION_ID, "/action_id"),
                (NATS_HEADER_BLOCK_ID, "/block_id"),
            ],
        };
        let mut headers = Vec::new();
        for (name, pointer) in sources {
            let identifier = payload
                .pointer(pointer)
                .and_then(Value::as_str)
                .filter(|identifier| !identifier.is_empty());
            if let Some(identifier) = identifier {
                headers.push((*name, HeaderText::new(identifier)?));
            }
        }
        Ok(Self(headers))
    }

    pub fn apply(self, headers: &mut HeaderMap) {
        for (name, value) in self.0 {
            headers.insert(name, value.value().clone());
        }
    }
}

#[cfg(test)]
mod tests;
