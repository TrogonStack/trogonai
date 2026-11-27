use super::{Architecture, CommandRuntime, RuntimeMount, elf_architecture, labels};
use crate::adapter::constants::{DIGEST_LABEL, NONCE_LABEL, OWNER_LABEL};

mod seed_writer_tests;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const MACHINE_X86_64: u16 = 62;
const MACHINE_AARCH64: u16 = 183;
const PT_LOAD: u32 = 1;
const PT_INTERP: u32 = 3;

fn put(bytes: &mut [u8], offset: usize, value: &[u8]) {
    bytes[offset..offset + value.len()].copy_from_slice(value);
}

/// A minimal little-endian 64-bit Linux ELF header followed by one program header.
fn elf(machine: u16, segment: u32) -> Vec<u8> {
    let mut bytes = vec![0; 120];
    put(&mut bytes, 0, b"\x7fELF");
    bytes[4] = 2;
    bytes[5] = 1;
    bytes[6] = 1;
    put(&mut bytes, 16, &2_u16.to_le_bytes());
    put(&mut bytes, 18, &machine.to_le_bytes());
    put(&mut bytes, 20, &1_u32.to_le_bytes());
    put(&mut bytes, 32, &64_u64.to_le_bytes());
    put(&mut bytes, 54, &56_u16.to_le_bytes());
    put(&mut bytes, 56, &1_u16.to_le_bytes());
    put(&mut bytes, 64, &segment.to_le_bytes());
    bytes
}

#[test]
fn static_executables_report_their_architecture() -> TestResult {
    assert_eq!(elf_architecture(&elf(MACHINE_X86_64, PT_LOAD))?, Architecture::X86_64);
    assert_eq!(elf_architecture(&elf(MACHINE_AARCH64, PT_LOAD))?, Architecture::Aarch64);
    Ok(())
}

#[test]
fn dynamic_or_malformed_executables_are_refused() {
    assert!(elf_architecture(&elf(MACHINE_X86_64, PT_INTERP)).is_err());
    assert!(elf_architecture(&elf(1, PT_LOAD)).is_err());
    assert!(elf_architecture(b"not an executable").is_err());
    assert!(elf_architecture(&[]).is_err());

    let mut big_endian = elf(MACHINE_X86_64, PT_LOAD);
    big_endian[5] = 2;
    assert!(elf_architecture(&big_endian).is_err());

    let mut abi = elf(MACHINE_X86_64, PT_LOAD);
    put(&mut abi, 20, &2_u32.to_le_bytes());
    assert!(elf_architecture(&abi).is_err());

    let mut no_headers = elf(MACHINE_X86_64, PT_LOAD);
    put(&mut no_headers, 56, &0_u16.to_le_bytes());
    assert!(elf_architecture(&no_headers).is_err());

    let mut past_end = elf(MACHINE_X86_64, PT_LOAD);
    put(&mut past_end, 32, &4096_u64.to_le_bytes());
    assert!(elf_architecture(&past_end).is_err());

    let mut truncated = elf(MACHINE_X86_64, PT_LOAD);
    truncated.truncate(63);
    assert!(elf_architecture(&truncated).is_err());
}

#[test]
fn ownership_labels_carry_the_owner_digest_and_registration() {
    let labels = labels("owner", "digest", "nonce");
    assert_eq!(labels.len(), 3);
    assert_eq!(labels.get(OWNER_LABEL).map(String::as_str), Some("owner"));
    assert_eq!(labels.get(DIGEST_LABEL).map(String::as_str), Some("digest"));
    assert_eq!(labels.get(NONCE_LABEL).map(String::as_str), Some("nonce"));
}

#[test]
fn loading_hashes_the_artifact_and_names_its_volume_after_the_digest() -> TestResult {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("supervisor");
    std::fs::write(&path, elf(MACHINE_X86_64, PT_LOAD))?;
    let runtime = CommandRuntime::load(&path, "owner".into())?;
    assert_eq!(runtime.digest().len(), 64);
    assert!(runtime.digest().bytes().all(|byte| byte.is_ascii_hexdigit()));
    assert!(runtime.volume_name().contains(runtime.digest()));
    assert_eq!(runtime.architecture, Architecture::X86_64);
    let other = CommandRuntime::load(&path, "owner".into())?;
    assert_eq!(other.digest(), runtime.digest());
    assert_ne!(other.volume_name(), runtime.volume_name());
    Ok(())
}

#[test]
fn loading_refuses_bad_owners_missing_files_and_non_executables() -> TestResult {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("supervisor");
    std::fs::write(&path, elf(MACHINE_X86_64, PT_LOAD))?;
    assert!(CommandRuntime::load(&path, String::new()).is_err());
    assert!(CommandRuntime::load(&path, "bad\nowner".into()).is_err());
    assert!(CommandRuntime::load(&path, "o".repeat(1025)).is_err());
    assert!(CommandRuntime::load(&directory.path().join("absent"), "owner".into()).is_err());
    assert!(CommandRuntime::load(directory.path(), "owner".into()).is_err());
    let text = directory.path().join("text");
    std::fs::write(&text, b"plain text")?;
    assert!(CommandRuntime::load(&text, "owner".into()).is_err());
    Ok(())
}

#[test]
fn a_runtime_mount_binds_its_volume_read_only_and_names_the_executable() {
    let mount = RuntimeMount {
        volume: "volume".into(),
        root: "/runtime".into(),
        digest: "digest".into(),
    };
    assert_eq!(mount.binding(), "volume:/runtime:ro,nocopy");
    assert_eq!(mount.executable(), "/runtime/supervisor");
}
