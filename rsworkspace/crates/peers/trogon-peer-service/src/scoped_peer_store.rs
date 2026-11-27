use trogon_peer::TenantId;
/// The adapter must confine every stream operation to this authenticated tenant.
pub trait ScopedPeerStore {
    fn tenant(&self) -> &TenantId;
}
