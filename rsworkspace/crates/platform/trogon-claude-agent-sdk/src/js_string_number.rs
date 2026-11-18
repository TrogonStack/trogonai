//! JavaScript `Number(text)` for a string, the one conversion every upstream string-to-number read goes through:
//! the bridge `worker_epoch`, the browser transport `Retry-After`, `Math.min` on a settings timeout, array
//! `length` assignment and the relational checks of the zod port.

use crate::session_prompt::js_trim;

/// `Number(text)`, the ECMAScript `StringToNumber`: JavaScript white space is trimmed, empty text is 0, a signed
/// decimal literal or `Infinity` reads as written, and an unsigned `0x`, `0o` or `0b` integer is rounded to the
/// nearest double. Anything else, numeric separators and a signed prefixed literal included, is `NaN`.
pub(crate) fn js_string_to_number(text: &str) -> f64 {
    let text = js_trim(text);
    if text.is_empty() {
        return 0.0;
    }
    let radix = match text.get(..2) {
        Some("0x" | "0X") => 16,
        Some("0o" | "0O") => 8,
        Some("0b" | "0B") => 2,
        _ => 0,
    };
    if radix != 0 {
        return text.get(2..).map_or(f64::NAN, |digits| radix_integer(digits, radix));
    }
    let (sign, unsigned) = match text.strip_prefix('-') {
        Some(rest) => (-1.0, rest),
        None => (1.0, text.strip_prefix('+').unwrap_or(text)),
    };
    if unsigned == "Infinity" {
        return sign * f64::INFINITY;
    }
    if is_decimal_literal(unsigned) {
        unsigned.parse::<f64>().map_or(f64::NAN, |value| sign * value)
    } else {
        f64::NAN
    }
}

/// The integer `digits` spell in `radix` (2, 8 or 16), rounded once to the nearest double with ties to even, as
/// `Number` rounds the exact value; `NaN` when there are no digits or one is not a digit of the radix. Folding the
/// digits into a double one at a time rounds at every step past 2^53 and can land one unit off.
fn radix_integer(digits: &str, radix: u32) -> f64 {
    if digits.is_empty() {
        return f64::NAN;
    }
    let bits_per_digit = radix.trailing_zeros();
    let mut significand: u64 = 0;
    let mut width = 0;
    let mut dropped: i32 = 0;
    let mut sticky = false;
    for character in digits.chars() {
        let Some(digit) = character.to_digit(radix) else {
            return f64::NAN;
        };
        for shift in (0..bits_per_digit).rev() {
            let bit = (digit >> shift) & 1 == 1;
            if width < u64::BITS {
                if width > 0 || bit {
                    significand = (significand << 1) | u64::from(bit);
                    width += 1;
                }
            } else {
                dropped = dropped.saturating_add(1);
                sticky |= bit;
            }
        }
    }
    // A double keeps the top 53 of the 64 bits held, so the lowest held bit sits below the rounding bit and can
    // stand for every dropped bit: it only matters when the rest is exactly half way.
    #[allow(
        clippy::cast_precision_loss,
        reason = "the conversion is the single round to nearest that Number applies"
    )]
    let rounded = (significand | u64::from(sticky)) as f64;
    rounded * 2f64.powi(dropped)
}

/// `digits [. digits] [exponent]` or `. digits [exponent]`, the unsigned decimal literal `Number` accepts.
fn is_decimal_literal(text: &str) -> bool {
    let digits = |part: &str| part.bytes().all(|byte| byte.is_ascii_digit());
    let (mantissa, exponent) = match text.split_once(['e', 'E']) {
        Some((mantissa, exponent)) => (mantissa, Some(exponent)),
        None => (text, None),
    };
    let (whole, fraction) = match mantissa.split_once('.') {
        Some((whole, fraction)) => (whole, Some(fraction)),
        None => (mantissa, None),
    };
    let has_digit = !whole.is_empty() || fraction.is_some_and(|fraction| !fraction.is_empty());
    digits(whole)
        && fraction.is_none_or(digits)
        && has_digit
        && exponent.is_none_or(|exponent| {
            let exponent = exponent.strip_prefix(['+', '-']).unwrap_or(exponent);
            !exponent.is_empty() && digits(exponent)
        })
}

#[cfg(test)]
#[path = "js_string_number_tests.rs"]
mod tests;
