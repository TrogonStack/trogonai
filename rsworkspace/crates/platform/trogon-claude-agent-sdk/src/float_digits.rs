//! The shortest decimal digits of a double, chosen as JavaScript `Number::toString` and Python `repr` choose them.

use serde_json::Value;

/// The shortest digits that read back as a positive finite `value`, closest to it and even on a tie, with the
/// position of the decimal point among them. `Number::toString`, Python `repr` and the printer `serde_json`
/// prints doubles with break a tie to the even digit; Rust's `{:e}` rounds it up, so 2^-25 would end in 3
/// instead of 2. The exponent `serde_json` prints may carry a `+`, which parsing it accepts.
pub(crate) fn shortest_digits(value: f64) -> (String, i32) {
    let printed = Value::from(value).to_string();
    let (mantissa, exponent) = printed.split_once('e').unwrap_or((printed.as_str(), "0"));
    let (whole, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let digits = format!("{whole}{fraction}");
    let leading = digits.len() - digits.trim_start_matches('0').len();
    let point = whole.len() as i32 + exponent.parse::<i32>().unwrap_or(0) - leading as i32;
    (digits.trim_matches('0').to_owned(), point)
}
