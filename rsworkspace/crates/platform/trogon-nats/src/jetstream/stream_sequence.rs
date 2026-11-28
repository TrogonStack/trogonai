/// Reads a JetStream message's stream sequence from its `$JS.ACK` reply subject.
///
/// Accepts both ADR-15 layouts, as `async_nats::jetstream::Message::info` does:
/// `$JS.ACK.<stream>.<consumer>.<delivered>.<stream_seq>.<consumer_seq>.<timestamp>.<pending>`,
/// and the longer one that adds the domain and account hash before the stream.
/// `None` for anything else, such as a message delivered without a reply.
pub fn stream_sequence(message: &async_nats::Message) -> Option<u64> {
    let tokens = message.reply.as_deref()?.strip_prefix("$JS.ACK.")?;
    let index = match tokens.matches('.').count() + 1 {
        7 => 3,
        n if n >= 9 => 5,
        _ => return None,
    };
    tokens.split('.').nth(index)?.parse().ok()
}

#[cfg(test)]
mod tests;
