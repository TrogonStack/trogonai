use tier3_skill_abi::{read_input, write_meta, write_output, AbiError};

const REFUSE: &[u8] = b"A2A_T3_REFUSE:UnauthorizedDataCategory";
const REFUSE_TRIGGER: &str = "SMOKE_T3_REFUSE_ME";

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
    if std::str::from_utf8(input)
        .ok()
        .is_some_and(|text| text.contains(REFUSE_TRIGGER))
    {
        return write_output(REFUSE);
    }
    write_output(input)
}
