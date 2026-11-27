/// What a verb is authenticated against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerbScope {
    /// The request names a lease; the key must belong to the lease's tenant.
    Lease,
    /// The request carries a `tenant`; the key must belong to that tenant.
    Tenant,
}
