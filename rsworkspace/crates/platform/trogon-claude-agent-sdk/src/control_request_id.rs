#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ControlRequestId(String);

impl ControlRequestId {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// TypeScript `Math.random().toString(36).substring(2,15)`: up to 13 base-36 digits of a random number in
    /// `[0, 1)`, with no counter behind it.
    pub(crate) fn typescript() -> Self {
        Self(typescript_id(math_random()))
    }

    /// Python `f"req_{counter}_{os.urandom(4).hex()}"`, where `counter` counts the requests sent so far,
    /// this one included.
    pub(crate) fn python(counter: u64) -> Self {
        let random = &uuid::Uuid::new_v4().as_u128().to_be_bytes()[12..];
        let mut hex = String::with_capacity(8);
        for byte in random {
            hex.push(char::from(DIGITS[usize::from(byte >> 4)]));
            hex.push(char::from(DIGITS[usize::from(byte & 0xf)]));
        }
        Self(format!("req_{counter}_{hex}"))
    }
}

const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";

/// V8 `Math.random()`: 52 random mantissa bits scaled into `[0, 1)`. The low 62 bits of a version 4 UUID are
/// all random.
fn math_random() -> f64 {
    let bits = (uuid::Uuid::new_v4().as_u128() & ((1 << 62) - 1)) >> 10;
    #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
    let value = bits as u64 as f64;
    value / 4_503_599_627_370_496.0
}

/// `value.toString(36).substring(2,15)` for `value` in `[0, 1)`.
pub(crate) fn typescript_id(value: f64) -> String {
    radix_36(value).chars().skip(2).take(13).collect()
}

/// V8 `DoubleToRadixCString` with radix 36, for a finite, non-negative `value` below 1: the shortest digit
/// string that reads back as `value`.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::float_cmp,
    clippy::indexing_slicing
)]
fn radix_36(value: f64) -> String {
    let mut integer = value.floor();
    let mut fraction = value - integer;
    let mut delta = 0.5 * (f64::from_bits(value.to_bits() + 1) - value);
    delta = delta.max(f64::from_bits(1));
    let mut digits: Vec<u8> = Vec::new();
    if fraction >= delta {
        loop {
            fraction *= 36.0;
            delta *= 36.0;
            let digit = fraction as usize;
            digits.push(DIGITS[digit]);
            fraction -= digit as f64;
            if (fraction > 0.5 || (fraction == 0.5 && digit & 1 == 1)) && fraction + delta > 1.0 {
                loop {
                    let Some(last) = digits.pop() else {
                        integer += 1.0;
                        break;
                    };
                    let digit = DIGITS
                        .iter()
                        .position(|candidate| *candidate == last)
                        .unwrap_or_default();
                    if digit + 1 < 36 {
                        digits.push(DIGITS[digit + 1]);
                        break;
                    }
                }
                break;
            }
            if fraction < delta {
                break;
            }
        }
    }
    let mut text = format!("{}", integer as u64);
    if !digits.is_empty() {
        text.push('.');
        text.extend(digits.into_iter().map(char::from));
    }
    text
}

impl From<String> for ControlRequestId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for ControlRequestId {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl std::fmt::Display for ControlRequestId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
#[path = "control_request_id_tests.rs"]
mod tests;
