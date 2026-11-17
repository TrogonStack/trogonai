/// Why a delivered credential stream message does not say where the stream
/// stored it.
///
/// The sequence and the append time come from the delivery's acknowledgement
/// subject, so a delivery without one, or with one this build cannot parse,
/// cannot become the notice its event may call for.
#[derive(Debug, thiserror::Error)]
#[error("the delivery does not say where the credential stream stored the message")]
pub struct DeliveryInfoError(#[source] pub(crate) async_nats::Error);

#[cfg(test)]
mod tests;
