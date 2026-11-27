use crate::OrdinaryCommandRuntimeError;

mod constants;

use constants::{ELF_HEADER_BYTES, MACHINE_ARM64, MACHINE_X64, MAX_PROGRAM_HEADERS, PROGRAM_HEADER_BYTES, PT_INTERP};

/// The machine architectures a command supervisor can be built for.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum GuestArchitecture {
    X64,
    Arm64,
}

fn le_u16(bytes: &[u8], offset: usize) -> Result<u16, OrdinaryCommandRuntimeError> {
    let field = bytes
        .get(offset..offset + 2)
        .ok_or(OrdinaryCommandRuntimeError::ArtifactTruncated)?;
    Ok(u16::from_le_bytes([field[0], field[1]]))
}

fn le_u32(bytes: &[u8], offset: usize) -> Result<u32, OrdinaryCommandRuntimeError> {
    let field = bytes
        .get(offset..offset + 4)
        .ok_or(OrdinaryCommandRuntimeError::ArtifactTruncated)?;
    Ok(u32::from_le_bytes([field[0], field[1], field[2], field[3]]))
}

fn le_u64(bytes: &[u8], offset: usize) -> Result<u64, OrdinaryCommandRuntimeError> {
    let field = bytes
        .get(offset..offset + 8)
        .ok_or(OrdinaryCommandRuntimeError::ArtifactTruncated)?;
    let mut word = [0; 8];
    word.copy_from_slice(field);
    Ok(u64::from_le_bytes(word))
}

impl GuestArchitecture {
    /// Reads the architecture of a little-endian ELF64 executable for Linux and refuses one that asks for an
    /// interpreter, since the sandbox may not have it.
    pub(crate) fn from_elf(bytes: &[u8]) -> Result<Self, OrdinaryCommandRuntimeError> {
        let identity = bytes
            .get(..ELF_HEADER_BYTES)
            .ok_or(OrdinaryCommandRuntimeError::NotLinuxElf64)?;
        let linux_elf64 = identity.starts_with(b"\x7fELF")
            && identity[4] == 2
            && identity[5] == 1
            && identity[6] == 1
            && matches!(identity[7], 0 | 3);
        if !linux_elf64 {
            return Err(OrdinaryCommandRuntimeError::NotLinuxElf64);
        }
        if !matches!(le_u16(bytes, 16)?, 2 | 3) || le_u32(bytes, 20)? != 1 {
            return Err(OrdinaryCommandRuntimeError::ElfAbiUnsupported);
        }
        let architecture = match le_u16(bytes, 18)? {
            MACHINE_X64 => Self::X64,
            MACHINE_ARM64 => Self::Arm64,
            _ => return Err(OrdinaryCommandRuntimeError::ArchitectureUnsupported),
        };
        let offset = usize::try_from(le_u64(bytes, 32)?).map_err(|_| OrdinaryCommandRuntimeError::ElfHeadersInvalid)?;
        let size = usize::from(le_u16(bytes, 54)?);
        let count = usize::from(le_u16(bytes, 56)?);
        if size != PROGRAM_HEADER_BYTES || !(1..=MAX_PROGRAM_HEADERS).contains(&count) {
            return Err(OrdinaryCommandRuntimeError::ElfHeadersInvalid);
        }
        let end = offset
            .checked_add(size * count)
            .ok_or(OrdinaryCommandRuntimeError::ElfHeadersInvalid)?;
        if end > bytes.len() {
            return Err(OrdinaryCommandRuntimeError::ArtifactTruncated);
        }
        for index in 0..count {
            if le_u32(bytes, offset + size * index)? == PT_INTERP {
                return Err(OrdinaryCommandRuntimeError::GuestInterpreter);
            }
        }
        Ok(architecture)
    }

    /// The architecture a Node process reports for itself.
    pub(crate) fn from_node(name: &str) -> Result<Self, OrdinaryCommandRuntimeError> {
        match name {
            "x64" => Ok(Self::X64),
            "arm64" => Ok(Self::Arm64),
            _ => Err(OrdinaryCommandRuntimeError::GuestArchitectureUnsupported),
        }
    }
}

#[cfg(test)]
mod tests;
