/// Which Fly Proxy network exposed ports are published on.
///
/// Fly Proxy cannot authenticate a request, and the instance header that pins a request to one Machine is not a
/// secret, so a port published on the public network is open to anyone who learns the Machine id. `Flycast`
/// publishes ports only on the app's private Flycast address, reachable from the organization's private network,
/// so the authenticated gateway preview is the only way in from outside.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, clap::ValueEnum)]
pub enum FlyPreviewNetwork {
    #[default]
    Public,
    Flycast,
}
