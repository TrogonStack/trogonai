mod redact;

use tier3_skill_abi::{read_input, write_meta, write_output, AbiError};

pub use redact::{redact_json_bytes, RedactError, SanitizerConfig};

#[unsafe(no_mangle)]
pub extern "C" fn redact_part_core(in_ptr: i32, in_len: i32, meta_ptr: i32) {
    let (out_ptr, out_len) = match redact_part_inner(in_ptr, in_len) {
        Ok(pair) => pair,
        Err(_) => (in_ptr, in_len),
    };
    write_meta(meta_ptr, out_ptr, out_len);
}

fn redact_part_inner(in_ptr: i32, in_len: i32) -> Result<(i32, i32), AbiError> {
    let input = read_input(in_ptr, in_len)?;
    let output = redact::redact_json_bytes(input).map_err(|_| AbiError::OutputTooLarge)?;
    write_output(&output)
}
