use std::collections::BTreeSet;
use std::fmt;

use trogonai_proto::peers::v1alpha1::{PeerBatch, PreparedMessage};

use crate::constants::NOTICE;
use crate::context_message::{ContextMessage, parse_identifier};
use crate::context_timestamp::ContextTimestamp;
use crate::peer_context_presentation::PeerContextPresentation;
use crate::{MAX_CONTEXT_BYTES, MAX_MESSAGES, PeerContextError};

#[derive(Clone, PartialEq, Eq)]
pub struct PeerContext(String);

impl fmt::Debug for PeerContext {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PeerContext")
            .field("utf8_bytes", &self.0.len())
            .finish_non_exhaustive()
    }
}

impl PeerContext {
    pub fn render(batch: &PeerBatch, messages: &[PreparedMessage]) -> Result<Self, PeerContextError> {
        if messages.is_empty() || messages.len() > MAX_MESSAGES || messages.len() != batch.message_ids.len() {
            return Err(PeerContextError::ContractViolation);
        }
        let batch_id = parse_identifier(batch.batch_id.as_deref())?;
        let prepared_at = batch
            .prepared_at
            .as_option()
            .ok_or(PeerContextError::ContractViolation)?;
        ContextTimestamp::from_wire(prepared_at)?;
        let parsed = messages
            .iter()
            .map(ContextMessage::from_wire)
            .collect::<Result<Vec<_>, _>>()?;
        let mut seen = BTreeSet::new();
        let first = parsed.first().ok_or(PeerContextError::ContractViolation)?;
        for (expected, message) in batch.message_ids.iter().zip(&parsed) {
            if expected != message.message_id()
                || !seen.insert(message.message_id())
                || first.recipient_peer_id() != message.recipient_peer_id()
                || first.collaboration_ref() != message.collaboration_ref()
            {
                return Err(PeerContextError::ContractViolation);
            }
        }
        let presentation = PeerContextPresentation {
            batch_id,
            messages: parsed.iter().map(ContextMessage::presentation).collect(),
        };
        let value = serde_json::to_value(presentation).map_err(PeerContextError::Encode)?;
        let json = serde_json::to_string(&value).map_err(PeerContextError::Encode)?;
        let escaped = json
            .replace('&', "\\u0026")
            .replace('<', "\\u003c")
            .replace('>', "\\u003e")
            .replace('\u{2028}', "\\u2028")
            .replace('\u{2029}', "\\u2029");
        let context = format!("{NOTICE}\n{escaped}");
        if context.len() > MAX_CONTEXT_BYTES {
            return Err(PeerContextError::ContractViolation);
        }
        Ok(Self(context))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}
