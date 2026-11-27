/// A warm VM a lease claimed from the pool, and the root DataVolume it boots from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WarmClaim {
    pub vm_name: String,
    pub dv_name: String,
}
