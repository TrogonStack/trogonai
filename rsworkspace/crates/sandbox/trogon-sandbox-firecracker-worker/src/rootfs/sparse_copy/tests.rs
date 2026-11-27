use super::*;
use std::os::unix::fs::MetadataExt;

fn allocated(path: &Path) -> Result<u64, Box<dyn std::error::Error>> {
    Ok(std::fs::metadata(path)?.blocks() * 512)
}

#[tokio::test]
async fn the_copy_is_byte_identical_and_leaves_zero_runs_as_holes() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let base = dir.path().join("base.ext4");
    let mut image = vec![0u8; 64 * 1024 * 1024];
    image[..BLOCK].fill(0xAB);
    image[10 * 1024 * 1024 + 17] = 1;
    let tail = image.len() - BLOCK / 2;
    image[tail..].fill(0xCD);
    image.truncate(image.len() - 100);
    std::fs::write(&base, &image)?;

    let copy = dir.path().join("lease.ext4");
    let length = copy_sparse(&base, &copy).await?;

    assert_eq!(length, image.len() as u64);
    assert_eq!(std::fs::read(&copy)?, image);
    assert!(
        allocated(&copy)? < allocated(&base)? / 4,
        "the copy allocated {} bytes where the dense base took {}",
        allocated(&copy)?,
        allocated(&base)?
    );
    Ok(())
}

#[tokio::test]
async fn data_between_the_holes_of_a_sparse_base_is_copied_exactly() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let base = dir.path().join("base.ext4");
    let file = File::create(&base)?;
    file.set_len(48 * 1024 * 1024)?;
    file.write_all_at(&[0x11; 3 * BLOCK + 9], 1024 * 1024 + 5)?;
    file.write_all_at(&[0x22; CHUNK + 1], 40 * 1024 * 1024)?;
    drop(file);

    let copy = dir.path().join("lease.ext4");
    copy_sparse(&base, &copy).await?;

    assert_eq!(std::fs::read(&copy)?, std::fs::read(&base)?);
    Ok(())
}

#[tokio::test]
async fn an_image_ending_in_zeros_keeps_its_full_length() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let base = dir.path().join("base.ext4");
    let mut image = vec![0u8; 3 * CHUNK + 5];
    image[0] = 7;
    std::fs::write(&base, &image)?;

    let copy = dir.path().join("lease.ext4");
    copy_sparse(&base, &copy).await?;

    assert_eq!(std::fs::read(&copy)?, image);
    Ok(())
}

#[tokio::test]
async fn a_stale_copy_is_replaced_rather_than_merged() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let base = dir.path().join("base.ext4");
    let image = vec![0u8; 2 * BLOCK];
    std::fs::write(&base, &image)?;
    let copy = dir.path().join("lease.ext4");
    std::fs::write(&copy, vec![0xEEu8; 4 * BLOCK])?;

    copy_sparse(&base, &copy).await?;

    assert_eq!(std::fs::read(&copy)?, image);
    Ok(())
}
