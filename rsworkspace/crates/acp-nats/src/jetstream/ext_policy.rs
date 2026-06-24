use crate::ext_method_name::ExtMethodName;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtPersistence {
    Stream,
    Ephemeral,
}

pub trait ExtStreamPolicy: Send + Sync {
    fn persistence(&self, method: &ExtMethodName) -> ExtPersistence;
}

pub struct DefaultExtStreamPolicy;

impl ExtStreamPolicy for DefaultExtStreamPolicy {
    fn persistence(&self, _method: &ExtMethodName) -> ExtPersistence {
        ExtPersistence::Stream
    }
}

#[cfg(test)]
mod tests;
