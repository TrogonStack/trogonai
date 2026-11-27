/// Whether the guest may reach the network.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum GuestNetwork {
    /// hluk's default: the guest connects nowhere.
    Denied,
    /// `hluk run --net`: every destination.
    Open,
}
