use crate::python_json::float_repr;

/// A `--max-budget-usd` amount, sent to the CLI the way the upstream SDKs send it.
///
/// Neither upstream SDK validates the amount: `UsdBudget::new` keeps any `f64`, including negative
/// and non-finite values, and the argv renders it like the selected SDK would. `UsdBudget::checked`
/// is an opt-in check that refuses negative and non-finite amounts. `UsdBudget::from_integer` keeps an
/// integer amount as one: Python's `max_budget_usd` accepts an `int` and sends `str(0)` as `0`, where the
/// float `0.0` is sent as `0.0`.
#[derive(Clone, Copy, Debug)]
pub struct UsdBudget(Amount);

#[derive(Clone, Copy, Debug)]
enum Amount {
    Float(f64),
    Integer(i64),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error("a USD budget must be finite and nonnegative")]
pub struct UsdBudgetError;

impl UsdBudget {
    pub fn new(value: f64) -> Self {
        Self(Amount::Float(value))
    }

    /// An integer amount, which the Python dialect prints the way `str(int)` does: `0` rather than `0.0`.
    pub fn from_integer(value: i64) -> Self {
        Self(Amount::Integer(value))
    }

    /// Opt-in check that refuses negative and non-finite amounts, which no upstream SDK rejects.
    pub fn checked(value: f64) -> Result<Self, UsdBudgetError> {
        if !value.is_finite() || value < 0.0 {
            return Err(UsdBudgetError);
        }
        Ok(Self::new(value))
    }

    pub fn get(self) -> f64 {
        match self.0 {
            Amount::Float(value) => value,
            // The nearest double, as JavaScript reads an integer amount.
            Amount::Integer(value) => value as f64,
        }
    }

    /// `String(value)` in JavaScript, which the TypeScript SDK uses for `--max-budget-usd=<value>`.
    pub(crate) fn typescript_text(self) -> String {
        match self.get() {
            value if value.is_nan() => "NaN".to_owned(),
            f64::INFINITY => "Infinity".to_owned(),
            f64::NEG_INFINITY => "-Infinity".to_owned(),
            value => crate::settings_json::js_float(value),
        }
    }

    /// `str(value)` in Python, which the Python SDK uses for `--max-budget-usd <value>`.
    pub(crate) fn python_text(self) -> String {
        match self.0 {
            Amount::Float(value) => python_float_repr(value),
            Amount::Integer(value) => value.to_string(),
        }
    }
}

/// Python's `==` between the amounts: an integer equals a float of exactly the same value.
impl PartialEq for UsdBudget {
    fn eq(&self, other: &Self) -> bool {
        match (self.0, other.0) {
            (Amount::Float(left), Amount::Float(right)) => left == right,
            (Amount::Integer(left), Amount::Integer(right)) => left == right,
            (Amount::Integer(integer), Amount::Float(float)) | (Amount::Float(float), Amount::Integer(integer)) => {
                float_equals_integer(float, integer)
            }
        }
    }
}

/// Whether `float` is exactly `integer`, without the rounding an `as` conversion either way would add.
fn float_equals_integer(float: f64, integer: i64) -> bool {
    const TWO_POW_63: f64 = 9_223_372_036_854_775_808.0;
    if !(-TWO_POW_63..TWO_POW_63).contains(&float) || float.fract() != 0.0 {
        return false;
    }
    // The range and fraction checks above make the conversion exact.
    float as i64 == integer
}

impl From<f64> for UsdBudget {
    fn from(value: f64) -> Self {
        Self::new(value)
    }
}

/// CPython `str(float)`, which is `float.__repr__`: `nan`, `inf` and `-inf` for the non-finite values, otherwise
/// the shortest round-trip digits with a tie kept on the even digit, as `json.dumps` prints a finite float.
fn python_float_repr(value: f64) -> String {
    if value.is_nan() {
        return "nan".to_owned();
    }
    if value.is_infinite() {
        return if value < 0.0 { "-inf" } else { "inf" }.to_owned();
    }
    float_repr(value)
}

#[cfg(test)]
#[path = "usd_budget_tests.rs"]
mod tests;
