//! Exact port of the arithmetic inside jsonschema 4.26 `multipleOf` (`_keywords.py`) on CPython 3.12.
//!
//! ```python
//! if isinstance(dB, float):
//!     quotient = instance / dB
//!     try:
//!         failed = int(quotient) != quotient
//!     except OverflowError:
//!         failed = (Fraction(instance) / Fraction(dB)).denominator != 1
//! else:
//!     failed = instance % dB
//! ```
//!
//! Every Python exception the expression can raise is surfaced as `Err(str(exception))`.
//!
//! Python `int` is [`PyNumber::Int`] inside the `i128` range and [`PyNumber::Big`] outside it. A `Big` past the
//! `float` range makes every mixed `int`/`float` operation raise `int too large to convert to float`.
//!
//! `Fraction` arithmetic is exact: every finite `f64` and every `int` is `m * 2^e` with `m` a [`Magnitude`], and
//! `a / b` is an integer exactly when the odd part of `b`'s mantissa divides the odd part of `a`'s mantissa and
//! the combined power of two is not negative.

use std::cmp::Ordering;

use crate::python_json_value::{ExactInteger, PythonJsonValue as Value, PythonNumber};

/// A Python `int` or `float` as the `multipleOf` keyword function sees it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum PyNumber {
    Int(i128),
    /// An `int` outside the `i128` range.
    Big(ExactInteger),
    Float(f64),
}

/// The schema value `dB`: a number, a Python `bool` (an `int` subclass), or any other type, which
/// only matters through its Python type name in the `TypeError` text.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum PyOperand {
    Number(PyNumber),
    Bool(bool),
    Other(&'static str),
}

/// The number as Python computes with it.
pub(crate) fn number(n: &PythonNumber) -> PyNumber {
    match n {
        PythonNumber::Int(integer) => integer
            .as_str()
            .parse()
            .map_or_else(|_| PyNumber::Big(integer.clone()), PyNumber::Int),
        PythonNumber::Float(float) => PyNumber::Float(*float),
    }
}

fn operand(value: &Value) -> PyOperand {
    match value {
        Value::Number(n) => PyOperand::Number(number(n)),
        Value::Bool(b) => PyOperand::Bool(*b),
        Value::String(_) => PyOperand::Other("str"),
        Value::Null => PyOperand::Other("NoneType"),
        Value::Array(_) => PyOperand::Other("list"),
        Value::Object(_) => PyOperand::Other("dict"),
    }
}

/// Runs jsonschema's `multipleOf` for `instance` against `divisor`.
///
/// `Ok(true)` means the keyword yields a `ValidationError`, `Ok(false)` that it does not (including
/// every non-number or `bool` instance, which `is_type(instance, "number")` skips), and `Err` the
/// `str()` of the Python exception the expression raises.
pub(crate) fn fails_multiple_of(instance: &Value, divisor: &Value) -> Result<bool, String> {
    match instance {
        Value::Number(n) => fails_multiple_of_numbers(number(n), operand(divisor)),
        _ => Ok(false),
    }
}

pub(crate) fn fails_multiple_of_numbers(instance: PyNumber, divisor: PyOperand) -> Result<bool, String> {
    match divisor {
        PyOperand::Number(PyNumber::Float(d)) => float_divisor(&instance, d),
        PyOperand::Number(d) => int_divisor(&instance, &d),
        PyOperand::Bool(b) => int_divisor(&instance, &PyNumber::Int(i128::from(b))),
        PyOperand::Other(name) => Err(format!(
            "unsupported operand type(s) for %: '{}' and '{name}'",
            if matches!(instance, PyNumber::Float(_)) {
                "float"
            } else {
                "int"
            }
        )),
    }
}

/// `float(n)`, which raises `OverflowError` for an `int` past the `float` range.
fn to_float(n: &PyNumber) -> Result<f64, String> {
    match n {
        PyNumber::Int(i) => Ok(*i as f64),
        PyNumber::Big(big) => Some(big.to_f64())
            .filter(|float| float.is_finite())
            .ok_or_else(|| "int too large to convert to float".to_owned()),
        PyNumber::Float(f) => Ok(*f),
    }
}

fn float_divisor(instance: &PyNumber, d: f64) -> Result<bool, String> {
    // CPython `float_div` converts both operands before it checks the divisor.
    let x = to_float(instance)?;
    if d == 0.0 {
        return Err("float division by zero".to_owned());
    }
    let quotient = x / d;
    if quotient.is_nan() {
        return Err("cannot convert float NaN to integer".to_owned());
    }
    if quotient.is_infinite() {
        // `int(quotient)` raised OverflowError: exact Fraction fallback.
        let (ma, ea) = decompose(instance)?;
        let (mb, eb) = decompose(&PyNumber::Float(d))?;
        return Ok(!fraction_quotient_is_integer(&ma, ea, &mb, eb));
    }
    Ok(quotient.trunc() != quotient)
}

/// `instance % d` for an `int` divisor `d`.
fn int_divisor(instance: &PyNumber, d: &PyNumber) -> Result<bool, String> {
    if let PyNumber::Float(x) = instance {
        // CPython `float_rem` converts both operands, then `mod = fmod(x, w)`; a non-zero `mod` of the wrong sign
        // gets `w` added (never reaching zero since |mod| < |w|) and a zero `mod` stays zero, so the truthiness
        // equals that of `fmod` itself (NaN is truthy).
        let w = to_float(d)?;
        if w == 0.0 {
            return Err("float modulo".to_owned());
        }
        return Ok(x % w != 0.0);
    }
    match (instance, d) {
        (_, PyNumber::Int(0)) => Err("integer modulo by zero".to_owned()),
        (PyNumber::Int(i), PyNumber::Int(d)) => Ok(i.wrapping_rem(*d) != 0),
        // `|i| < |d|` for a `Big` divisor, so `i % d` is zero only for a zero `i`.
        (PyNumber::Int(i), _) => Ok(*i != 0),
        _ => Ok(!decompose(instance)?.0.is_multiple_of(&decompose(d)?.0)),
    }
}

/// `Fraction(n)` as `(mantissa, exponent)` with `|n| = mantissa * 2^exponent`.
fn decompose(n: &PyNumber) -> Result<(Magnitude, i32), String> {
    match n {
        PyNumber::Int(i) => Ok((Magnitude::from(i.unsigned_abs()), 0)),
        PyNumber::Big(big) => Ok((Magnitude::from_decimal(big.as_str().trim_start_matches('-')), 0)),
        PyNumber::Float(f) if f.is_infinite() => Err("cannot convert Infinity to integer ratio".to_owned()),
        PyNumber::Float(f) if f.is_nan() => Err("cannot convert NaN to integer ratio".to_owned()),
        PyNumber::Float(f) => {
            let bits = f.to_bits();
            let biased = ((bits >> 52) & 0x7ff) as i32;
            let fraction = u128::from(bits & ((1_u64 << 52) - 1));
            if biased == 0 {
                Ok((Magnitude::from(fraction), -1074))
            } else {
                Ok((Magnitude::from(fraction | (1_u128 << 52)), biased - 1075))
            }
        }
    }
}

/// `(Fraction(a) / Fraction(b)).denominator == 1` for `a = ma * 2^ea` and non-zero `b = mb * 2^eb`.
fn fraction_quotient_is_integer(ma: &Magnitude, ea: i32, mb: &Magnitude, eb: i32) -> bool {
    if ma.is_zero() {
        return true;
    }
    let sa = ma.trailing_zeros();
    let sb = mb.trailing_zeros();
    if !ma.shr(sa).is_multiple_of(&mb.shr(sb)) {
        return false;
    }
    // `odd_a / odd_b` is odd, so the power of two must not be a division.
    i64::from(ea) + sa as i64 - i64::from(eb) - sb as i64 >= 0
}

/// Python `a <=> b` for two `int`s.
pub(crate) fn int_cmp(a: &ExactInteger, b: &ExactInteger) -> Ordering {
    match (a.is_negative(), b.is_negative()) {
        (false, true) => Ordering::Greater,
        (true, false) => Ordering::Less,
        (negative, _) => {
            let (x, y) = (a.as_str().trim_start_matches('-'), b.as_str().trim_start_matches('-'));
            let order = x.len().cmp(&y.len()).then_with(|| x.cmp(y));
            if negative { order.reverse() } else { order }
        }
    }
}

/// Python `big <=> f` for an `int` outside the `i128` range, exact as CPython's `float_richcompare` is; `None`
/// when `f` is NaN, which orders against nothing.
pub(crate) fn big_cmp_float(big: &ExactInteger, f: f64) -> Option<Ordering> {
    if f.is_nan() {
        return None;
    }
    let outside = if big.is_negative() {
        Ordering::Less
    } else {
        Ordering::Greater
    };
    let two_pow_127 = (1_u128 << 127) as f64;
    // Every float in `[-2**127, 2**127)`, and every float of the other sign, lies on the near side of `big`.
    if f.is_sign_negative() != big.is_negative() || (-two_pow_127..two_pow_127).contains(&f) {
        return Some(outside);
    }
    if f.is_infinite() {
        return Some(outside.reverse());
    }
    let (mantissa, exponent) = decompose(&PyNumber::Float(f)).ok()?;
    let float = mantissa.shl(usize::try_from(exponent).ok()?);
    let order = Magnitude::from_decimal(big.as_str().trim_start_matches('-')).cmp(&float);
    Some(if big.is_negative() { order.reverse() } else { order })
}

/// A non-negative integer of any size: little-endian `u32` limbs without a high zero limb.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Magnitude(Vec<u32>);

impl From<u128> for Magnitude {
    fn from(mut value: u128) -> Self {
        let mut limbs = Vec::new();
        while value != 0 {
            limbs.push(value as u32);
            value >>= 32;
        }
        Self(limbs)
    }
}

impl Magnitude {
    /// The value of the ASCII decimal `digits`.
    pub(crate) fn from_decimal(digits: &str) -> Self {
        let mut limbs: Vec<u32> = Vec::new();
        for digit in digits.bytes() {
            let mut carry = u64::from(digit.saturating_sub(b'0'));
            for limb in &mut limbs {
                let next = u64::from(*limb) * 10 + carry;
                *limb = next as u32;
                carry = next >> 32;
            }
            if carry != 0 {
                limbs.push(carry as u32);
            }
        }
        Self(limbs)
    }

    pub(crate) fn is_zero(&self) -> bool {
        self.0.is_empty()
    }

    fn limb(&self, index: usize) -> u32 {
        self.0.get(index).copied().unwrap_or(0)
    }

    fn bits(&self) -> usize {
        self.0
            .last()
            .map_or(0, |top| self.0.len() * 32 - top.leading_zeros() as usize)
    }

    fn bit(&self, index: usize) -> bool {
        (self.limb(index / 32) >> (index % 32)) & 1 == 1
    }

    fn normalized(mut self) -> Self {
        self.trim();
        self
    }

    /// Drops the high zero limbs.
    fn trim(&mut self) {
        while self.0.last() == Some(&0) {
            self.0.pop();
        }
    }

    pub(crate) fn trailing_zeros(&self) -> usize {
        let mut zeros = 0;
        for limb in &self.0 {
            if *limb != 0 {
                return zeros + limb.trailing_zeros() as usize;
            }
            zeros += 32;
        }
        0
    }

    /// `self >> bits`.
    pub(crate) fn shr(&self, bits: usize) -> Self {
        let (skip, rest) = (bits / 32, bits % 32);
        let limbs = (skip..self.0.len())
            .map(|index| {
                let wide = (u64::from(self.limb(index + 1)) << 32) | u64::from(self.limb(index));
                (wide >> rest) as u32
            })
            .collect();
        Self(limbs).normalized()
    }

    /// `self << bits`.
    pub(crate) fn shl(&self, bits: usize) -> Self {
        let (skip, rest) = (bits / 32, bits % 32);
        let mut limbs = vec![0; skip];
        let mut carry = 0;
        for limb in &self.0 {
            let wide = (u64::from(*limb) << rest) | carry;
            limbs.push(wide as u32);
            carry = wide >> 32;
        }
        limbs.push(carry as u32);
        Self(limbs).normalized()
    }

    /// `self -= other` for `other <= self`.
    fn subtract(&mut self, other: &Self) {
        let mut borrow = 0;
        for (index, limb) in self.0.iter_mut().enumerate() {
            let difference = i64::from(*limb) - i64::from(other.limb(index)) - borrow;
            borrow = i64::from(difference < 0);
            *limb = (difference + (borrow << 32)) as u32;
        }
        self.trim();
    }

    /// Whether `divisor` divides `self`, by binary long division; a zero divisor divides only zero.
    pub(crate) fn is_multiple_of(&self, divisor: &Self) -> bool {
        if divisor.is_zero() {
            return self.is_zero();
        }
        let mut remainder = Self(Vec::new());
        for index in (0..self.bits()).rev() {
            remainder = remainder.shl(1);
            if self.bit(index) {
                match remainder.0.first_mut() {
                    Some(low) => *low |= 1,
                    None => remainder.0.push(1),
                }
            }
            if remainder >= *divisor {
                remainder.subtract(divisor);
            }
        }
        remainder.is_zero()
    }
}

impl Ord for Magnitude {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0
            .len()
            .cmp(&other.0.len())
            .then_with(|| self.0.iter().rev().cmp(other.0.iter().rev()))
    }
}

impl PartialOrd for Magnitude {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[cfg(test)]
#[path = "python_arith_tests.rs"]
mod tests;
