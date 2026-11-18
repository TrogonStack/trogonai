use super::*;

#[test]
fn reads_numbers_like_the_number_function() {
    assert_eq!(js_string_to_number(""), 0.0);
    assert_eq!(js_string_to_number("  12 "), 12.0);
    assert_eq!(js_string_to_number("1e3"), 1000.0);
    assert_eq!(js_string_to_number(".5"), 0.5);
    assert_eq!(js_string_to_number("0x1f"), 31.0);
    assert_eq!(js_string_to_number("0O17"), 15.0);
    assert_eq!(js_string_to_number("0b11"), 3.0);
    assert_eq!(js_string_to_number("Infinity"), f64::INFINITY);
    assert_eq!(js_string_to_number("+Infinity"), f64::INFINITY);
    assert_eq!(js_string_to_number("-Infinity"), f64::NEG_INFINITY);
    for text in ["0x", "0xg", "abc", "1_0", "--1", "infinity", "1e"] {
        assert!(js_string_to_number(text).is_nan(), "{text}");
    }
}

/// The expected values are what `Number(text)` returns under Node 24, which rounds the exact value of a prefixed
/// literal once. Folding the digits into a double one at a time gives 2^57, 2^60 and 2^54 for the first three.
#[test]
fn rounds_a_prefixed_literal_past_two_to_the_fifty_three_once() {
    let power = |exponent: i32| 2f64.powi(exponent);
    let cases = [
        ("0x200000000000011", power(57) + 32.0),
        ("0o100000000000000000201", power(60) + 256.0),
        (
            "0b1000000000000000000000000000000000000000000000000000011",
            power(54) + 4.0,
        ),
        ("0x20000000000001", power(53)),
        ("0x1fffffffffffff", power(53) - 1.0),
        ("0x10000000000000000", power(64)),
        ("0x0000000000000000000000001", 1.0),
    ];
    for (text, expected) in cases {
        assert_eq!(js_string_to_number(text).to_bits(), f64::to_bits(expected), "{text}");
    }
}

#[test]
fn a_prefixed_literal_past_the_double_range_is_infinite() {
    let largest = format!("0xfffffffffffffb{}", "f".repeat(242));
    assert_eq!(js_string_to_number(&largest), f64::MAX);
    let half_way = format!("0xfffffffffffffc{}", "0".repeat(242));
    assert_eq!(js_string_to_number(&half_way), f64::INFINITY);
    assert_eq!(js_string_to_number(&format!("0x{}", "f".repeat(300))), f64::INFINITY);
    assert_eq!(js_string_to_number(&format!("0b{}", "1".repeat(2000))), f64::INFINITY);
}

#[test]
fn a_prefixed_literal_takes_no_sign_separator_or_inner_space() {
    for text in ["0x+1", "0x-1", "+0x1", "-0x1", "0x1_0", "0x 1", "0 x1", "0b1.1", "1n"] {
        assert!(js_string_to_number(text).is_nan(), "{text}");
    }
    assert_eq!(js_string_to_number(" 0x10 "), 16.0);
}

#[test]
fn reads_decimal_literals_and_white_space_like_the_number_function() {
    assert_eq!(js_string_to_number("1e400"), f64::INFINITY);
    assert_eq!(js_string_to_number("-1e400"), f64::NEG_INFINITY);
    assert_eq!(js_string_to_number("1e-400"), 0.0);
    assert!(js_string_to_number("-0").is_sign_negative());
    assert_eq!(js_string_to_number("00012"), 12.0);
    assert_eq!(js_string_to_number("+.5"), 0.5);
    assert_eq!(js_string_to_number("5."), 5.0);
    assert_eq!(js_string_to_number("1.e3"), 1000.0);
    assert_eq!(js_string_to_number("1E+2"), 100.0);
    assert_eq!(
        js_string_to_number("\t\n\u{b}\u{c}\r\u{2028}\u{2029}\u{3000}\u{feff} 7 "),
        7.0
    );
    assert_eq!(js_string_to_number("  "), 0.0);
    for text in ["1e+", ".", "+", "e5", "\u{85} 7", "\u{200b}7", "1e5e5", "NaN", "inf"] {
        assert!(js_string_to_number(text).is_nan(), "{text:?}");
    }
}
