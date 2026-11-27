use super::*;

/// A minimal ELF64 header with one program header of `program_type`.
fn elf(machine: u16, program_type: u32) -> Vec<u8> {
    let mut bytes = vec![0u8; 64 + 56];
    bytes[..4].copy_from_slice(b"\x7fELF");
    bytes[4] = 2;
    bytes[5] = 1;
    bytes[6] = 1;
    bytes[16..18].copy_from_slice(&2u16.to_le_bytes());
    bytes[18..20].copy_from_slice(&machine.to_le_bytes());
    bytes[20..24].copy_from_slice(&1u32.to_le_bytes());
    bytes[32..40].copy_from_slice(&64u64.to_le_bytes());
    bytes[54..56].copy_from_slice(&56u16.to_le_bytes());
    bytes[56..58].copy_from_slice(&1u16.to_le_bytes());
    bytes[64..68].copy_from_slice(&program_type.to_le_bytes());
    bytes
}

#[test]
fn static_x64_and_arm64_executables_are_accepted() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(GuestArchitecture::from_elf(&elf(62, 1))?, GuestArchitecture::X64);
    assert_eq!(GuestArchitecture::from_elf(&elf(183, 1))?, GuestArchitecture::Arm64);
    Ok(())
}

#[test]
fn an_executable_that_asks_for_an_interpreter_is_refused() {
    assert!(matches!(
        GuestArchitecture::from_elf(&elf(62, 3)),
        Err(OrdinaryCommandRuntimeError::GuestInterpreter)
    ));
}

#[test]
fn other_machines_and_other_formats_are_refused() {
    assert!(matches!(
        GuestArchitecture::from_elf(&elf(40, 1)),
        Err(OrdinaryCommandRuntimeError::ArchitectureUnsupported)
    ));
    assert!(matches!(
        GuestArchitecture::from_elf(b"#!/bin/sh\n"),
        Err(OrdinaryCommandRuntimeError::NotLinuxElf64)
    ));
    let mut big_endian = elf(62, 1);
    big_endian[5] = 2;
    assert!(matches!(
        GuestArchitecture::from_elf(&big_endian),
        Err(OrdinaryCommandRuntimeError::NotLinuxElf64)
    ));
}

#[test]
fn a_truncated_program_header_table_is_refused() {
    let mut bytes = elf(62, 1);
    bytes.truncate(100);
    assert!(matches!(
        GuestArchitecture::from_elf(&bytes),
        Err(OrdinaryCommandRuntimeError::ArtifactTruncated)
    ));
}

#[test]
fn bad_program_header_geometry_is_refused() {
    let mut bytes = elf(62, 1);
    bytes[54..56].copy_from_slice(&40u16.to_le_bytes());
    assert!(matches!(
        GuestArchitecture::from_elf(&bytes),
        Err(OrdinaryCommandRuntimeError::ElfHeadersInvalid)
    ));
}

#[test]
fn an_unknown_abi_is_refused() {
    let mut bytes = elf(62, 1);
    bytes[16..18].copy_from_slice(&1u16.to_le_bytes());
    assert!(matches!(
        GuestArchitecture::from_elf(&bytes),
        Err(OrdinaryCommandRuntimeError::ElfAbiUnsupported)
    ));
}

#[test]
fn node_names_map_to_architectures() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(GuestArchitecture::from_node("x64")?, GuestArchitecture::X64);
    assert_eq!(GuestArchitecture::from_node("arm64")?, GuestArchitecture::Arm64);
    assert!(GuestArchitecture::from_node("ia32").is_err());
    Ok(())
}
