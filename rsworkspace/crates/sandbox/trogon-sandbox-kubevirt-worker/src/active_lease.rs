use crate::LeaseResources;

/// What the provider remembers about one lease it holds.
#[derive(Clone)]
pub(crate) struct ActiveLease {
    pub(crate) resources: LeaseResources,
    pub(crate) vm_name: String,
}
