use uuid::Uuid;

#[cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    expect(constant_outside_constants_module, reason = "the constant is private to this module")
)]
const CROCKFORD: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// A fresh time-ordered identifier in canonical ULID text form: 26 uppercase Crockford base32
/// characters whose first character is `0` through `7`.
pub(crate) fn generated_ulid() -> String {
    encode(Uuid::now_v7().as_u128())
}

pub(crate) fn encode(value: u128) -> String {
    let mut out = [b'0'; 26];
    let mut rest = value;
    for slot in out.iter_mut().rev() {
        let index = usize::from(u8::try_from(rest & 0x1f).unwrap_or(0));
        *slot = CROCKFORD[index];
        rest >>= 5;
    }
    out.iter().map(|byte| char::from(*byte)).collect()
}

#[cfg(test)]
mod tests;
