use super::*;

/// The expected values are what `Number(value)` returned under Node 24 for the same JSON values: an array reads
/// as the text `String(array)` joins, so only an empty or single element array can be a number.
#[test]
fn converts_json_values_like_the_number_function() -> Result<(), serde_json::Error> {
    let cases = [
        ("[]", 0.0),
        (r#"[" 0x10 "]"#, 16.0),
        ("[[5]]", 5.0),
        ("[null]", 0.0),
        ("2.5", 2.5),
        ("true", 1.0),
        ("null", 0.0),
        (r#""0x200000000000011""#, 2f64.powi(57) + 32.0),
    ];
    for (text, expected) in cases {
        assert_eq!(
            to_number(&serde_json::from_str(text)?).to_bits(),
            f64::to_bits(expected),
            "{text}"
        );
    }
    for text in ["[1,2]", r#"["0x+1"]"#, "[true]", "[{}]", "{}"] {
        assert!(to_number(&serde_json::from_str(text)?).is_nan(), "{text}");
    }
    Ok(())
}
