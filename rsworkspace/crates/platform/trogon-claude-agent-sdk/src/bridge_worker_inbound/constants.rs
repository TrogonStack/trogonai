/// How many uuids each of the two sets remembers (`x9`).
pub const INBOUND_DEDUP_CAPACITY: usize = 2_000;

/// The only `origin` a queued notification may carry (`Kb`).
pub const NOTIFICATION_ORIGIN: &str = "mcp_send_message";
