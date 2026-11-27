use super::*;

#[cfg(target_os = "linux")]
#[test]
fn an_inherited_descriptor_does_not_outlive_the_image_cache_owner() -> Result<(), Box<dyn std::error::Error>> {
    let parent = tempfile::tempdir()?;
    let config = OciRootfsConfig {
        cache_dir: parent.path().join("cache"),
        guest_agent: "unused".into(),
        guest_init: "unused".into(),
        umoci: "unused".into(),
        mkfs_ext4: "unused".into(),
        disk_size_bytes: 1024 * 1024,
        max_cache_bytes: 2 * 1024 * 1024,
        max_download_bytes: 1024 * 1024,
    };
    let oci = Arc::new(OciClient::new(Default::default()));
    let owner = ImageCache::new(config.clone(), oci.clone());
    owner.lock_root()?;
    let inherited = owner
        .process_lock
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_ref()
        .ok_or("value missing")?
        .1
        .try_clone()?;
    drop(owner);
    let replacement = ImageCache::new(config, oci);
    let result = replacement.lock_root();
    drop(inherited);
    assert!(result.is_ok(), "the image cache guard must own its lock lifetime");
    Ok(())
}

fn elf(machine: u16, dynamic: bool) -> Vec<u8> {
    let mut data = vec![0u8; if dynamic { 200 } else { 121 }];
    data[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
    data[16..18].copy_from_slice(&2u16.to_le_bytes());
    data[18..20].copy_from_slice(&machine.to_le_bytes());
    data[20..24].copy_from_slice(&1u32.to_le_bytes());
    data[24..32].copy_from_slice(&0x400078u64.to_le_bytes());
    data[32..40].copy_from_slice(&64u64.to_le_bytes());
    data[52..54].copy_from_slice(&64u16.to_le_bytes());
    data[54..56].copy_from_slice(&56u16.to_le_bytes());
    data[56..58].copy_from_slice(&(if dynamic { 2u16 } else { 1u16 }).to_le_bytes());
    data[64..68].copy_from_slice(&1u32.to_le_bytes());
    data[68..72].copy_from_slice(&5u32.to_le_bytes());
    data[80..88].copy_from_slice(&0x400000u64.to_le_bytes());
    let len = data.len() as u64;
    data[96..104].copy_from_slice(&len.to_le_bytes());
    data[104..112].copy_from_slice(&len.to_le_bytes());
    if dynamic {
        data[120..124].copy_from_slice(&3u32.to_le_bytes());
        data[128..136].copy_from_slice(&176u64.to_le_bytes());
        data[152..160].copy_from_slice(&8u64.to_le_bytes());
        data[176..184].copy_from_slice(b"/loader\0");
    }
    data
}

#[test]
fn injected_binaries_require_static_amd64_loadable_elf() -> Result<(), Box<dyn std::error::Error>> {
    validate_guest_binary(&elf(62, false))?;
    assert!(validate_guest_binary(&elf(183, false)).is_err());
    assert!(validate_guest_binary(&elf(62, true)).is_err());
    assert!(validate_guest_binary(b"\x7fELF").is_err());
    Ok(())
}

#[test]
fn cached_bases_are_bound_to_image_and_injected_binary_content() -> Result<(), Box<dyn std::error::Error>> {
    let image = Digest::new(format!("sha256:{}", "1".repeat(64)))?;
    let other = Digest::new(format!("sha256:{}", "2".repeat(64)))?;
    let size = 128 * 1024 * 1024;
    let key = cache_identity(&image, b"agent", b"init", size)?;
    assert_ne!(key, cache_identity(&other, b"agent", b"init", size)?);
    assert_ne!(key, cache_identity(&image, b"new-agent", b"init", size)?);
    assert_ne!(key, cache_identity(&image, b"agent", b"new-init", size)?);
    assert_ne!(key, cache_identity(&image, b"agent", b"init", size * 2)?);
    Ok(())
}
