#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BindingVersion(u32);

impl BindingVersion {
    pub fn new(value: u32) -> Result<Self, BindingVersionError> {
        if value != 1 {
            return Err(BindingVersionError);
        }
        Ok(Self(value))
    }

    pub fn get(self) -> u32 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("unsupported binding version")]
pub struct BindingVersionError;
