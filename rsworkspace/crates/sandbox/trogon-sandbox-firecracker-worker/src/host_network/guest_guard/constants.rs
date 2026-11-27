/// Name of the table that holds the TAP ingress chain and its counters.
pub(crate) const TABLE: &str = "sandbox_fc_l2";
/// Counter incremented for every frame the guard drops, whatever the reason.
pub(crate) const SPOOF_COUNTER: &str = "spoofed";
