use super::*;
use crate::python_json_value::PythonJsonObject;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const FIXTURE: &str = include_str!("../tests/fixtures/sdk_mcp/python_arith.json");

fn field<'a>(case: &'a serde_json::Value, name: &str) -> Result<&'a str, String> {
    case.get(name)
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| format!("missing {name}"))
}

#[test]
fn replays_every_python_multiple_of_case() -> TestResult {
    let cases: Vec<serde_json::Value> = serde_json::from_str(FIXTURE)?;
    assert!(cases.len() >= 300);
    let (mut failed, mut passed, mut errors) = (0, 0, 0);
    for case in &cases {
        let instance = Value::from(serde_json::from_str::<serde_json::Value>(field(case, "instance")?)?);
        let divisor = Value::from(serde_json::from_str::<serde_json::Value>(field(case, "divisor")?)?);
        let got = fails_multiple_of(&instance, &divisor);
        match case.get("error").and_then(serde_json::Value::as_str) {
            Some(text) => {
                errors += 1;
                assert_eq!(got, Err(text.to_owned()), "{case}");
            }
            None => {
                let expected = case.get("failed").and_then(serde_json::Value::as_bool);
                assert_eq!(got.as_ref().ok().copied(), expected, "{case}");
                if expected == Some(true) {
                    failed += 1;
                } else {
                    passed += 1;
                }
            }
        }
    }
    assert!(failed > 20 && passed > 20 && errors > 20);
    Ok(())
}

#[test]
fn non_number_and_bool_instances_are_skipped() {
    for instance in [
        Value::Null,
        Value::Bool(true),
        Value::from("a"),
        Value::Array(Vec::new()),
    ] {
        assert_eq!(fails_multiple_of(&instance, &Value::from(0)), Ok(false));
    }
}

#[test]
fn operand_types_map_to_python_type_names() {
    assert_eq!(operand(&Value::Bool(false)), PyOperand::Bool(false));
    assert_eq!(operand(&Value::Null), PyOperand::Other("NoneType"));
    assert_eq!(operand(&Value::from("")), PyOperand::Other("str"));
    assert_eq!(operand(&Value::Array(Vec::new())), PyOperand::Other("list"));
    assert_eq!(
        operand(&Value::Object(PythonJsonObject::new())),
        PyOperand::Other("dict")
    );
    assert_eq!(
        operand(&Value::from(u64::MAX)),
        PyOperand::Number(PyNumber::Int(i128::from(u64::MAX)))
    );
    assert_eq!(
        operand(&Value::from(i64::MIN)),
        PyOperand::Number(PyNumber::Int(i128::from(i64::MIN)))
    );
    assert_eq!(operand(&Value::from(0.5)), PyOperand::Number(PyNumber::Float(0.5)));
}

#[test]
fn infinite_and_nan_floats_raise_the_python_errors() {
    let int = PyNumber::Int;
    let float = PyNumber::Float;
    let by = |d: f64| PyOperand::Number(float(d));
    assert_eq!(
        fails_multiple_of_numbers(float(f64::INFINITY), by(2.0)),
        Err("cannot convert Infinity to integer ratio".to_owned())
    );
    assert_eq!(
        fails_multiple_of_numbers(float(f64::NEG_INFINITY), by(2.0)),
        Err("cannot convert Infinity to integer ratio".to_owned())
    );
    assert_eq!(
        fails_multiple_of_numbers(float(f64::INFINITY), by(f64::INFINITY)),
        Err("cannot convert float NaN to integer".to_owned())
    );
    assert_eq!(
        fails_multiple_of_numbers(float(f64::NAN), by(2.0)),
        Err("cannot convert float NaN to integer".to_owned())
    );
    assert_eq!(fails_multiple_of_numbers(int(3), by(f64::INFINITY)), Ok(false));
    assert_eq!(
        decompose(&float(f64::NAN)),
        Err("cannot convert NaN to integer ratio".to_owned())
    );
    assert_eq!(
        fails_multiple_of_numbers(float(f64::NAN), PyOperand::Number(int(2))),
        Ok(true)
    );
    assert_eq!(
        fails_multiple_of_numbers(float(-3.0), PyOperand::Number(int(-2))),
        Ok(true)
    );
    assert_eq!(
        fails_multiple_of_numbers(float(-4.0), PyOperand::Number(int(-2))),
        Ok(false)
    );
}

#[test]
fn integer_modulo_handles_the_extremes() {
    let int = PyNumber::Int;
    assert_eq!(
        fails_multiple_of_numbers(int(i128::MIN), PyOperand::Number(int(-1))),
        Ok(false)
    );
    assert_eq!(fails_multiple_of_numbers(int(-7), PyOperand::Bool(true)), Ok(false));
    assert_eq!(
        fails_multiple_of_numbers(int(-7), PyOperand::Bool(false)),
        Err("integer modulo by zero".to_owned())
    );
    assert_eq!(
        fails_multiple_of_numbers(PyNumber::Float(1.5), PyOperand::Bool(false)),
        Err("float modulo".to_owned())
    );
}

#[test]
fn exact_fraction_fallback_for_overflowing_quotients() {
    let by = |d: f64| PyOperand::Number(PyNumber::Float(d));
    // 1 / 5e-324 overflows a float quotient; the exact quotient is the integer 2**1074.
    assert_eq!(fails_multiple_of_numbers(PyNumber::Int(1), by(5e-324)), Ok(false));
    assert_eq!(decompose(&PyNumber::Float(5e-324)), Ok((Magnitude::from(1), -1074)));
    assert_eq!(
        decompose(&PyNumber::Float(1.0)),
        Ok((Magnitude::from(1_u128 << 52), -52))
    );
    assert_eq!(fails_multiple_of_numbers(PyNumber::Float(1e308), by(1e-5)), Ok(true));
    let m = Magnitude::from;
    assert!(fraction_quotient_is_integer(&m(0), 0, &m(1), 0));
    assert!(fraction_quotient_is_integer(&m(4), 0, &m(1), 0));
    assert!(fraction_quotient_is_integer(&m(3), 0, &m(3), 0));
    assert!(!fraction_quotient_is_integer(&m(3), 0, &m(5), 0));
    assert!(!fraction_quotient_is_integer(&m(1), 0, &m(1), 1));
    assert!(fraction_quotient_is_integer(&m(1), 1, &m(1), 1));
}

/// `2**n` in decimal: Rust prints a whole `f64` exactly.
fn two_pow(n: i32) -> String {
    format!("{:.0}", 2_f64.powi(n))
}

fn big(digits: &str) -> Result<PyNumber, Box<dyn std::error::Error>> {
    Ok(PyNumber::Big(digits.parse()?))
}

const TWO_POW_200_PLUS_1: &str = "1606938044258990275541962092341162602522202993782792835301377";

/// Each answer below is CPython 3.12's for the same operands.
#[test]
fn integers_outside_i128_take_the_python_paths() -> TestResult {
    let ten_pow_400 = format!("1{}", "0".repeat(400));
    let int = |i: i128| PyOperand::Number(PyNumber::Int(i));
    let by = |d: f64| PyOperand::Number(PyNumber::Float(d));
    assert_eq!(number(&PythonNumber::Int(ten_pow_400.parse()?)), big(&ten_pow_400)?);
    assert_eq!(number(&PythonNumber::Int(two_pow(127).parse()?)), big(&two_pow(127))?);
    let minimum = (-(1_i128 << 126) * 2).to_string();
    assert_eq!(number(&PythonNumber::Int(minimum.parse()?)), PyNumber::Int(i128::MIN));
    let huge = PyOperand::Number(big(&ten_pow_400)?);
    assert_eq!(fails_multiple_of_numbers(PyNumber::Int(3), huge.clone()), Ok(true));
    assert_eq!(fails_multiple_of_numbers(PyNumber::Int(0), huge.clone()), Ok(false));
    let too_large = Err("int too large to convert to float".to_owned());
    assert_eq!(fails_multiple_of_numbers(PyNumber::Float(1.5), huge), too_large);
    assert_eq!(fails_multiple_of_numbers(big(&ten_pow_400)?, by(2.0)), too_large);
    let below = PyOperand::Number(big("-170141183460469231731687303715884105729")?);
    assert_eq!(
        fails_multiple_of_numbers(PyNumber::Float(2_f64.powi(127)), below),
        Ok(false)
    );
    assert_eq!(fails_multiple_of_numbers(big(&two_pow(200))?, int(1 << 64)), Ok(false));
    assert_eq!(fails_multiple_of_numbers(big(TWO_POW_200_PLUS_1)?, int(2)), Ok(true));
    let by_big = PyOperand::Number(big(&two_pow(200))?);
    assert_eq!(
        fails_multiple_of_numbers(big(&format!("{:.0}", 3.0 * 2_f64.powi(200)))?, by_big),
        Ok(false)
    );
    assert_eq!(
        fails_multiple_of_numbers(big(&two_pow(130))?, by(0.0)),
        Err("float division by zero".to_owned())
    );
    // `2**1000 / (3 * 2**-40)` overflows a float, so the exact Fraction decides.
    let third = 3.0 * 2_f64.powi(-40);
    assert_eq!(fails_multiple_of_numbers(big(&two_pow(1000))?, by(third)), Ok(true));
    let thrice = format!("{:.0}", 3.0 * 2_f64.powi(1000));
    assert_eq!(fails_multiple_of_numbers(big(&thrice)?, by(third)), Ok(false));
    Ok(())
}

#[test]
fn magnitudes_compute_exactly() {
    let two_pow_128 = Magnitude::from(1).shl(128);
    assert_eq!(Magnitude::from_decimal(&two_pow(128)), two_pow_128);
    assert_eq!(two_pow_128.trailing_zeros(), 128);
    assert_eq!(two_pow_128.shr(127), Magnitude::from(2));
    assert_eq!(two_pow_128.shr(129), Magnitude::from(0));
    assert!(Magnitude::from_decimal("0").is_zero());
    assert_eq!(Magnitude::from(0).trailing_zeros(), 0);
    assert_eq!(Magnitude::from_decimal("4294967296"), Magnitude::from(1_u128 << 32));
    let ten_pow_400 = Magnitude::from_decimal(&format!("1{}", "0".repeat(400)));
    let five_pow_400 = ten_pow_400.shr(400);
    assert!(ten_pow_400.is_multiple_of(&five_pow_400));
    assert!(ten_pow_400.is_multiple_of(&two_pow_128));
    assert!(!ten_pow_400.is_multiple_of(&Magnitude::from(3)));
    assert!(!five_pow_400.is_multiple_of(&Magnitude::from(2)));
    assert!(!Magnitude::from(1).is_multiple_of(&Magnitude::from(0)));
    assert!(Magnitude::from(0).is_multiple_of(&Magnitude::from(0)));
    assert!(five_pow_400 < ten_pow_400);
    assert!(Magnitude::from(u128::MAX) < two_pow_128);
    assert!(Magnitude::from(1_u128 << 64) > Magnitude::from(u128::from(u64::MAX)));
}

#[test]
fn ints_outside_i128_order_against_ints_and_floats_exactly() -> TestResult {
    let int = |digits: &str| digits.parse::<ExactInteger>();
    assert_eq!(int_cmp(&int("-5")?, &int("3")?), Ordering::Less);
    assert_eq!(int_cmp(&int("1")?, &int("-1")?), Ordering::Greater);
    assert_eq!(int_cmp(&int("-10")?, &int("-9")?), Ordering::Less);
    assert_eq!(int_cmp(&int("100")?, &int("99")?), Ordering::Greater);
    assert_eq!(int_cmp(&int("42")?, &int("42")?), Ordering::Equal);
    let two_pow_200 = int(&two_pow(200))?;
    let float = 2_f64.powi(200);
    assert_eq!(big_cmp_float(&two_pow_200, float), Some(Ordering::Equal));
    assert_eq!(big_cmp_float(&int(TWO_POW_200_PLUS_1)?, float), Some(Ordering::Greater));
    let below = int("1606938044258990275541962092341162602522202993782792835301375")?;
    assert_eq!(big_cmp_float(&below, float), Some(Ordering::Less));
    let two_pow_127 = int(&two_pow(127))?;
    assert_eq!(big_cmp_float(&two_pow_127, 2_f64.powi(127)), Some(Ordering::Equal));
    assert_eq!(big_cmp_float(&two_pow_127, -0.0), Some(Ordering::Greater));
    assert_eq!(big_cmp_float(&two_pow_127, f64::INFINITY), Some(Ordering::Less));
    assert_eq!(big_cmp_float(&two_pow_127, f64::NAN), None);
    let negative = int("-170141183460469231731687303715884105729")?;
    assert_eq!(big_cmp_float(&negative, -(2_f64.powi(127))), Some(Ordering::Less));
    assert_eq!(big_cmp_float(&negative, f64::NEG_INFINITY), Some(Ordering::Greater));
    assert_eq!(big_cmp_float(&negative, -(2_f64.powi(200))), Some(Ordering::Greater));
    Ok(())
}
