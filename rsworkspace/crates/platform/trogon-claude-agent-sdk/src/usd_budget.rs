/// A `--max-budget-usd` amount, sent to the CLI the way the upstream SDKs send it.
///
/// Neither upstream SDK validates the amount: `UsdBudget::new` keeps any `f64`, including negative
/// and non-finite values, and the argv renders it like the selected SDK would. `UsdBudget::checked`
/// is an opt-in check that refuses negative and non-finite amounts.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UsdBudget(f64);

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error("a USD budget must be finite and nonnegative")]
pub struct UsdBudgetError;

impl UsdBudget {
    pub fn new(value: f64) -> Self {
        Self(value)
    }

    /// Opt-in check that refuses negative and non-finite amounts, which no upstream SDK rejects.
    pub fn checked(value: f64) -> Result<Self, UsdBudgetError> {
        if !value.is_finite() || value < 0.0 {
            return Err(UsdBudgetError);
        }
        Ok(Self(value))
    }

    pub fn get(self) -> f64 {
        self.0
    }

    /// `String(value)` in JavaScript, which the TypeScript SDK uses for `--max-budget-usd=<value>`.
    pub(crate) fn typescript_text(self) -> String {
        match self.0 {
            value if value.is_nan() => "NaN".to_owned(),
            f64::INFINITY => "Infinity".to_owned(),
            f64::NEG_INFINITY => "-Infinity".to_owned(),
            value => crate::session_fork_ts::js_float(value),
        }
    }

    /// `str(value)` in Python, which the Python SDK uses for `--max-budget-usd <value>`.
    pub(crate) fn python_text(self) -> String {
        python_float_repr(self.0)
    }
}

impl From<f64> for UsdBudget {
    fn from(value: f64) -> Self {
        Self::new(value)
    }
}

/// CPython `float.__repr__`: the shortest round-trip digits, fixed notation when the decimal point
/// falls between `1e-4` and `1e16`, scientific notation with a two-digit exponent otherwise.
fn python_float_repr(value: f64) -> String {
    if value.is_nan() {
        return "nan".to_owned();
    }
    if value.is_infinite() {
        return if value < 0.0 { "-inf" } else { "inf" }.to_owned();
    }
    let sign = if value.is_sign_negative() { "-" } else { "" };
    if value == 0.0 {
        return format!("{sign}0.0");
    }
    let scientific = format!("{:e}", value.abs());
    let Some((mantissa, exponent)) = scientific.split_once('e') else {
        return scientific;
    };
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    let point = exponent.parse::<i32>().map_or(1, |exponent| exponent + 1);
    let count = digits.len() as i32;
    let body = if -4 < point && point <= 16 {
        if point <= 0 {
            format!("0.{}{digits}", "0".repeat(point.unsigned_abs() as usize))
        } else if count <= point {
            format!("{digits}{}.0", "0".repeat((point - count) as usize))
        } else {
            format!("{}.{}", &digits[..point as usize], &digits[point as usize..])
        }
    } else {
        let power = point - 1;
        let sign_char = if power < 0 { '-' } else { '+' };
        let fraction = if count > 1 {
            format!(".{}", &digits[1..])
        } else {
            String::new()
        };
        format!("{}{fraction}e{sign_char}{:02}", &digits[..1], power.unsigned_abs())
    };
    format!("{sign}{body}")
}

#[cfg(test)]
#[path = "usd_budget_tests.rs"]
mod tests;
