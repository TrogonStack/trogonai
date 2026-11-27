use std::fmt;

use uuid::Uuid;

use crate::constants::{ULID_ALPHABET, ULID_BITS, ULID_LEN, ULID_MASK};
use crate::ulid_error::UlidError;

/// A 128-bit identifier written as 26 Crockford base32 characters, time ordered when generated.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub(crate) struct Ulid(u128);

impl Ulid {
    /// A fresh identifier built from a version 7 UUID.
    pub(crate) fn generate() -> Self {
        Self(Uuid::now_v7().as_u128())
    }

    /// Parses 26 Crockford base32 characters in either case.
    pub(crate) fn parse(value: &str) -> Result<Self, UlidError> {
        if value.len() != ULID_LEN {
            return Err(UlidError::Length);
        }
        let mut number = 0u128;
        for (index, byte) in value.bytes().enumerate() {
            let digit = ULID_ALPHABET
                .iter()
                .position(|candidate| *candidate == byte.to_ascii_uppercase())
                .ok_or(UlidError::InvalidCharacter)?;
            if index == 0 && digit > 7 {
                return Err(UlidError::Overflow);
            }
            number = (number << ULID_BITS) | digit as u128;
        }
        Ok(Self(number))
    }
}

impl fmt::Display for Ulid {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut text = [0u8; ULID_LEN];
        for (index, slot) in text.iter_mut().enumerate() {
            let shift = ULID_BITS * (ULID_LEN - 1 - index) as u32;
            let digit = ((self.0 >> shift) & ULID_MASK) as usize;
            *slot = ULID_ALPHABET.get(digit).copied().unwrap_or(b'0');
        }
        formatter.write_str(&String::from_utf8_lossy(&text))
    }
}
