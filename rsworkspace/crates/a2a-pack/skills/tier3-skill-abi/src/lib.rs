const META_OFFSET: i32 = 0;

pub fn read_input(in_ptr: i32, in_len: i32) -> Result<&'static [u8], AbiError> {
    if in_ptr < 0 || in_len < 0 {
        return Err(AbiError::InvalidBounds);
    }
    let ptr = usize::try_from(in_ptr).map_err(|_| AbiError::InvalidBounds)?;
    let len = usize::try_from(in_len).map_err(|_| AbiError::InvalidBounds)?;
    if len == 0 {
        return Ok(&[]);
    }
    let end = ptr.checked_add(len).ok_or(AbiError::InvalidBounds)?;
    if end > memory_len() {
        return Err(AbiError::InvalidBounds);
    }
    Ok(unsafe { core::slice::from_raw_parts(ptr as *const u8, len) })
}

pub fn write_output(bytes: &[u8]) -> Result<(i32, i32), AbiError> {
    let out_len = i32::try_from(bytes.len()).map_err(|_| AbiError::OutputTooLarge)?;
    let out_base = OUTPUT_OFFSET;
    let end = usize::try_from(out_base)
        .ok()
        .and_then(|base| base.checked_add(bytes.len()))
        .ok_or(AbiError::OutputTooLarge)?;
    if end > memory_len() {
        return Err(AbiError::OutputTooLarge);
    }
    unsafe {
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), out_base as *mut u8, bytes.len());
    }
    Ok((out_base, out_len))
}

pub fn write_meta(meta_ptr: i32, out_ptr: i32, out_len: i32) {
    unsafe {
        *(meta_ptr as *mut i32) = out_ptr;
        *((meta_ptr.wrapping_add(4)) as *mut i32) = out_len;
    }
}

pub fn passthrough_meta(meta_ptr: i32, in_ptr: i32, in_len: i32) {
    write_meta(meta_ptr, in_ptr, in_len);
}

const OUTPUT_OFFSET: i32 = 0x1000;

fn memory_len() -> usize {
    #[cfg(target_arch = "wasm32")]
    {
        return core::arch::wasm32::memory_size(0) * 65536;
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        65536
    }
}

pub const META_SCRATCH_OFFSET: i32 = META_OFFSET;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AbiError {
    InvalidBounds,
    OutputTooLarge,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_negative_bounds() {
        assert_eq!(read_input(-1, 4), Err(AbiError::InvalidBounds));
        assert_eq!(read_input(0, -1), Err(AbiError::InvalidBounds));
    }

    #[test]
    fn passthrough_meta_round_trip() {
        let mut storage = vec![0i32; 2];
        let ptr = storage.as_mut_ptr() as usize;
        if ptr > i32::MAX as usize {
            return;
        }
        passthrough_meta(ptr as i32, 42, 7);
        assert_eq!(storage, [42, 7]);
    }

    #[test]
    fn meta_scratch_offset_is_zero() {
        assert_eq!(META_SCRATCH_OFFSET, 0);
    }
}
