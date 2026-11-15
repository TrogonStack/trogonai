use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

/// An owned ephemeral data key. The caller keeps it inside its trusted material boundary.
///
/// ```compile_fail
/// use trogon_key_management::Dek;
/// use zeroize::Zeroizing;
/// let key = Dek::from_bytes(Zeroizing::new([0; 32]));
/// let copied_key = key.clone();
/// ```
pub struct Dek(Zeroizing<[u8; 32]>);

impl Dek {
    pub fn new(bytes: Zeroizing<Vec<u8>>) -> Result<Self, DekError> {
        let mut material = Zeroizing::new([0; 32]);
        if bytes.len() != 32 {
            return Err(DekError);
        }
        material.copy_from_slice(&bytes);
        Ok(Self(material))
    }
    pub fn from_bytes(bytes: Zeroizing<[u8; 32]>) -> Self {
        Self(bytes)
    }
    /// Borrow only for the admitted cryptographic operation, never for logging or retention.
    pub fn expose(&self) -> &[u8; 32] {
        &self.0
    }
}

impl Zeroize for Dek {
    fn zeroize(&mut self) {
        self.0.zeroize();
    }
}
impl ZeroizeOnDrop for Dek {}

impl std::fmt::Debug for Dek {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Dek([REDACTED])")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("DEK must contain exactly 32 bytes")]
pub struct DekError;
