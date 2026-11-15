/// Whether a tenant holds a credential owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OwnerHolding {
    /// No tenant is known by that account.
    UnknownTenant,
    /// The tenant is known and does not hold the owner.
    NotHeld,
    /// The tenant holds the owner.
    Held,
}
