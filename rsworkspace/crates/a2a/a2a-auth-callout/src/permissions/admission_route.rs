/// How a caller reaches the bus, which decides the subjects a
/// [`super::CallerGrant`] opens to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdmissionRoute {
    /// The bridge minted the caller's user on the mint exchange and relays the
    /// caller's requests through the gateway.
    BridgeRouted,
    /// nats-server asked the callout to admit the caller's own connection.
    Direct,
}
