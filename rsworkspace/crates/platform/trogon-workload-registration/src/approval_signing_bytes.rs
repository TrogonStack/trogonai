use crate::{ProducerConfig, SIGNING_CONTEXT};

pub fn approval_signing_bytes(config: &ProducerConfig, payload: &[u8]) -> Vec<u8> {
    let domain = config.scope().trust_domain().as_str().as_bytes();
    let mut message = Vec::with_capacity(SIGNING_CONTEXT.len() + 2 + domain.len() + payload.len());
    message.extend_from_slice(SIGNING_CONTEXT);
    message.extend_from_slice(&(domain.len() as u16).to_be_bytes());
    message.extend_from_slice(domain);
    message.extend_from_slice(payload);
    message
}
