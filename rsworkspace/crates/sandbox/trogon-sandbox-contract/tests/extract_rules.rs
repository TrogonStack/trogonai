use serde_json::error::Category::Data;
use serde_json::json;
use trogon_sandbox_contract::{
    ClaimName, ClaimNameError, EnvVarName, EnvVarNameError, Extract, ExtractPattern, ExtractPatternError, JsonKeyPath,
    JsonKeyPathError, JwtMasking, RegexSyntaxError,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn error_of<T, E>(result: Result<T, E>) -> Result<E, Box<dyn std::error::Error>> {
    result.err().ok_or_else(|| "expected an error".into())
}

fn rejects<T: serde::de::DeserializeOwned>(value: serde_json::Value) -> TestResult {
    assert_eq!(error_of(serde_json::from_value::<T>(value.clone()))?.classify(), Data);
    assert_eq!(
        error_of(serde_json::from_str::<T>(&value.to_string()))?.classify(),
        Data
    );
    Ok(())
}

#[test]
fn patterns_that_ecmascript_accepts_with_a_group_are_accepted() -> TestResult {
    for pattern in [
        r"token:\s*(\S+)",
        r"(?<=token=)(\w+)",
        r"token=(?=(\w+))",
        "(a)(?!b)",
        "(?<!x)(a)",
        r"(a)\1",
        r"(?<n>a)\k<n>",
        r"(a)\2",
        r"\q(a)",
        "(a){,2}",
        "x{2(b)",
        r"\u{2}(b)",
        r"[\u{41}](a)",
        "(?i:a)(b)",
        "(a*)b",
        "(a+?)",
        "(?=a)*(b)",
        r"[\d-z](a)",
        r"\cA(a)",
        r"(\$\w+)",
        "(.)",
        "(?<n>a)|(?<n>b)",
        "(a){2,}",
        "(a){2,3}?",
        "😀(a)",
        r"[😀](a)",
        r"😀(a)",
        r"(?<a>a)",
        r"(?<\u{61}>a)",
        r"(?<\u{1d49c}>a)",
        r"(a)[A-Z]",
        r"(a)a{1,x}",
        r"(a)[A-Z]",
        r"(a)\ud83d",
        r"(a)😀",
        r"(a)A",
        r"(a)[A-Z]",
        r"(a)[\u00zz-z]",
        r"(?<$_a1‌>a)",
        r"(?i-s:a)(b)",
        r"(?-i:a)(b)",
        r"(a)\8",
        r"(a)\08",
        r"(a)\0",
        r"(a)\012",
        r"(a)\400",
        r"(a)\c",
        r"(a)\cz",
        r"(a)\xzz\x41",
        r"(a)\uzzzzA",
        r"(a)\u{",
        r"(a)\b\B",
        r"[\b\f\n\r\t\v\0](a)",
        r"[\cJ\c_\c1](a)",
        r"[\c](a)",
        r"[\x41\xzz](a)",
        r"[A\uzzzz](a)",
        r"[\012\7](a)",
        r"[\w\s\D-\d](a)",
        r"[a-c\-](a)",
        r"[a-](a)",
        r"[^\]a](b)",
        r"[\q](a)",
        r"(a)[\k]",
    ] {
        assert!(ExtractPattern::new(pattern).is_ok(), "{pattern}");
    }
    Ok(())
}

#[test]
fn capture_groups_are_required_the_way_ecmascript_counts_them() -> TestResult {
    for pattern in [
        r"token:\s*\S+",
        "(?:token)",
        r"(?<=token=)\w+",
        r"(?=a)(?!b)\1",
        "abc",
        "(?<=a)(?<!b)c",
        "[(a)]",
        r"\(a\)",
        "",
    ] {
        assert_eq!(
            error_of(ExtractPattern::new(pattern))?,
            ExtractPatternError::WithoutCapture,
            "{pattern}"
        );
    }
    Ok(())
}

#[test]
fn refused_patterns_read_as_a_javascript_engine_words_them() -> TestResult {
    let cases: &[(&str, RegexSyntaxError, &str)] = &[
        ("(a", RegexSyntaxError::UnterminatedGroup, "Unterminated group"),
        ("a)(b)", RegexSyntaxError::UnmatchedParenthesis, "Unmatched ')'"),
        (
            r"(?<n>a)\k<m>",
            RegexSyntaxError::InvalidNamedCapture,
            "Invalid named capture referenced",
        ),
        (
            "(?<n>a)(?<n>b)",
            RegexSyntaxError::DuplicateGroupName,
            "Duplicate capture group name",
        ),
        (
            "(?<n>a)(?<n>b)(?<n>c)",
            RegexSyntaxError::DuplicateGroupName,
            "Duplicate capture group name",
        ),
        (
            "(?:(?<n>a)|b)(?<n>c)",
            RegexSyntaxError::DuplicateGroupName,
            "Duplicate capture group name",
        ),
        ("(?<=a)*(b)", RegexSyntaxError::InvalidQuantifier, "Invalid quantifier"),
        ("(?<!a)+(b)", RegexSyntaxError::InvalidQuantifier, "Invalid quantifier"),
        (
            "(a){2,1}",
            RegexSyntaxError::QuantifierOutOfOrder,
            "numbers out of order in {} quantifier",
        ),
        (
            "(?ii:a)(b)",
            RegexSyntaxError::RepeatedFlag,
            "Repeated flag in flag group",
        ),
        (
            "(?i-i:a)(b)",
            RegexSyntaxError::RepeatedFlag,
            "Repeated flag in flag group",
        ),
        ("(?-:a)(b)", RegexSyntaxError::InvalidFlagGroup, "Invalid flag group"),
        (
            "[z-a](b)",
            RegexSyntaxError::ClassRangeOutOfOrder,
            "Range out of order in character class",
        ),
        (
            "(?<1a>x)",
            RegexSyntaxError::InvalidGroupName,
            "Invalid capture group name",
        ),
        (
            r"(?<n>a)\k<>",
            RegexSyntaxError::InvalidGroupName,
            "Invalid capture group name",
        ),
        ("(?<n", RegexSyntaxError::InvalidGroupName, "Invalid capture group name"),
        (
            r"(?<\u{110000}>a)",
            RegexSyntaxError::InvalidGroupName,
            "Invalid capture group name",
        ),
        (
            r"(?<\u{}>a)",
            RegexSyntaxError::InvalidGroupName,
            "Invalid capture group name",
        ),
        (
            r"(?<\u{61>a)",
            RegexSyntaxError::InvalidGroupName,
            "Invalid capture group name",
        ),
        (
            r"(?<\u{ffffffffff}>a)",
            RegexSyntaxError::InvalidGroupName,
            "Invalid capture group name",
        ),
        (
            r"(?<\u00zz>a)",
            RegexSyntaxError::InvalidGroupName,
            "Invalid capture group name",
        ),
        (
            r"(?<n😀>a)",
            RegexSyntaxError::InvalidGroupName,
            "Invalid capture group name",
        ),
        (
            r"(?<\q>a)",
            RegexSyntaxError::InvalidGroupName,
            "Invalid capture group name",
        ),
        (
            r"(?<\ud800>a)",
            RegexSyntaxError::InvalidGroupName,
            "Invalid capture group name",
        ),
        (
            "(?<a-b>a)",
            RegexSyntaxError::InvalidGroupName,
            "Invalid capture group name",
        ),
        (
            r"(?<n>a)\k",
            RegexSyntaxError::InvalidNamedReference,
            "Invalid named reference",
        ),
        (r"(?<n>a)[\k]", RegexSyntaxError::InvalidEscape, "Invalid escape"),
        (r"(a)\", RegexSyntaxError::TrailingBackslash, "\\ at end of pattern"),
        (r"(a)[\", RegexSyntaxError::TrailingBackslash, "\\ at end of pattern"),
        (r"(a)[a-\", RegexSyntaxError::TrailingBackslash, "\\ at end of pattern"),
        (
            r"(?<\u{100000000}>a)",
            RegexSyntaxError::InvalidGroupName,
            "Invalid capture group name",
        ),
        ("*(a)", RegexSyntaxError::NothingToRepeat, "Nothing to repeat"),
        ("(a)**", RegexSyntaxError::NothingToRepeat, "Nothing to repeat"),
        ("^*(a)", RegexSyntaxError::NothingToRepeat, "Nothing to repeat"),
        (r"\b+(a)", RegexSyntaxError::NothingToRepeat, "Nothing to repeat"),
        ("{1}(a)", RegexSyntaxError::NothingToRepeat, "Nothing to repeat"),
        ("(?a)(b)", RegexSyntaxError::InvalidGroup, "Invalid group"),
        (
            "[a",
            RegexSyntaxError::UnterminatedClass,
            "Unterminated character class",
        ),
        ("[", RegexSyntaxError::UnterminatedClass, "Unterminated character class"),
        (
            "(a)[a-",
            RegexSyntaxError::UnterminatedClass,
            "Unterminated character class",
        ),
        ("(a)(?<=b", RegexSyntaxError::UnterminatedGroup, "Unterminated group"),
    ];
    for (pattern, reason, message) in cases {
        let error = error_of(ExtractPattern::new(*pattern))?;
        assert_eq!(
            error,
            ExtractPatternError::Invalid {
                pattern: (*pattern).to_string(),
                reason: *reason
            },
            "{pattern}"
        );
        assert_eq!(reason.to_string(), *message, "{pattern}");
    }
    Ok(())
}

#[test]
fn a_pattern_the_engine_cannot_compile_is_unsupported() -> TestResult {
    let nested = format!("{}a{}(b)", "(?:".repeat(1000), ")".repeat(1000));
    assert_eq!(
        error_of(ExtractPattern::new(nested.clone()))?,
        ExtractPatternError::Invalid {
            pattern: nested,
            reason: RegexSyntaxError::Unsupported
        }
    );
    Ok(())
}

#[test]
fn error_messages_quote_the_pattern_like_a_regular_expression_literal() -> TestResult {
    let cases: &[(ExtractPatternError, &str)] = &[
        (
            error_of(ExtractPattern::new("a/("))?,
            "extract is not a valid regular expression: Invalid regular expression: /a\\/(/: Unterminated group",
        ),
        (
            error_of(ExtractPattern::new("a\\/(b"))?,
            "extract is not a valid regular expression: Invalid regular expression: /a\\/(b/: Unterminated group",
        ),
        (
            error_of(ExtractPattern::new("(?i)(a)"))?,
            "extract is not a valid regular expression: Invalid regular expression: /(?i)(a)/: Invalid group",
        ),
        (
            error_of(ExtractPattern::new("abc"))?,
            "extract must contain at least one capturing group - group 1 is the credential value to mask (e.g. \"token:\\s*(\\S+)\").",
        ),
    ];
    for (error, message) in cases {
        assert_eq!(error.to_string(), *message);
    }
    Ok(())
}

#[test]
fn syntax_error_messages_are_exact() {
    let cases: &[(RegexSyntaxError, &str)] = &[
        (RegexSyntaxError::UnterminatedGroup, "Unterminated group"),
        (RegexSyntaxError::UnmatchedParenthesis, "Unmatched ')'"),
        (RegexSyntaxError::NothingToRepeat, "Nothing to repeat"),
        (RegexSyntaxError::UnterminatedClass, "Unterminated character class"),
        (
            RegexSyntaxError::ClassRangeOutOfOrder,
            "Range out of order in character class",
        ),
        (
            RegexSyntaxError::QuantifierOutOfOrder,
            "numbers out of order in {} quantifier",
        ),
        (RegexSyntaxError::TrailingBackslash, "\\ at end of pattern"),
        (RegexSyntaxError::InvalidGroup, "Invalid group"),
        (RegexSyntaxError::InvalidFlagGroup, "Invalid flag group"),
        (RegexSyntaxError::RepeatedFlag, "Repeated flag in flag group"),
        (RegexSyntaxError::InvalidQuantifier, "Invalid quantifier"),
        (RegexSyntaxError::InvalidEscape, "Invalid escape"),
        (RegexSyntaxError::InvalidGroupName, "Invalid capture group name"),
        (RegexSyntaxError::InvalidNamedReference, "Invalid named reference"),
        (
            RegexSyntaxError::InvalidNamedCapture,
            "Invalid named capture referenced",
        ),
        (RegexSyntaxError::DuplicateGroupName, "Duplicate capture group name"),
        (RegexSyntaxError::Unsupported, "Invalid regular expression"),
    ];
    for (error, message) in cases {
        assert_eq!(error.to_string(), *message);
    }
}

#[test]
fn an_extract_pattern_round_trips_as_a_string() -> TestResult {
    let pattern: ExtractPattern = r"token:\s*(\S+)".parse()?;
    assert_eq!(pattern.as_str(), r"token:\s*(\S+)");
    assert_eq!(pattern.to_string(), r"token:\s*(\S+)");
    let wire = serde_json::to_value(&pattern)?;
    assert_eq!(wire, json!(r"token:\s*(\S+)"));
    assert_eq!(serde_json::from_value::<ExtractPattern>(wire)?, pattern);
    assert_eq!(ExtractPattern::try_from(r"(a)".to_string())?.as_str(), "(a)");
    assert_eq!(String::from(pattern), r"token:\s*(\S+)");
    assert!(error_of("no group".parse::<ExtractPattern>())? == ExtractPatternError::WithoutCapture);
    rejects::<ExtractPattern>(json!("no group"))?;
    rejects::<ExtractPattern>(json!("(unclosed"))?;
    rejects::<ExtractPattern>(json!(7))
}

#[test]
fn env_var_names_follow_shell_rules() -> TestResult {
    for valid in ["GH_TOKEN", "_x1", "a"] {
        let name: EnvVarName = valid.parse()?;
        assert_eq!(name.as_str(), valid);
        assert_eq!(name.to_string(), valid);
        assert_eq!(serde_json::to_value(&name)?, json!(valid));
        assert_eq!(serde_json::from_value::<EnvVarName>(json!(valid))?, name);
        assert_eq!(String::from(name), valid);
    }
    EnvVarName::new("A".repeat(300))?;
    for invalid in ["1X", "A-B", "", "A B", "é"] {
        assert_eq!(
            error_of(EnvVarName::new(invalid))?,
            EnvVarNameError::Invalid(invalid.to_string())
        );
        rejects::<EnvVarName>(json!(invalid))?;
    }
    assert_eq!(
        EnvVarName::try_from("1X".to_string()).err(),
        Some(EnvVarNameError::Invalid("1X".to_string()))
    );
    assert_eq!(
        error_of(EnvVarName::new("1X"))?.to_string(),
        "environment variable name '1X' must start with a letter or underscore and contain only letters, digits and underscores"
    );
    Ok(())
}

#[test]
fn claim_names_are_non_empty() -> TestResult {
    let name: ClaimName = "sub".parse()?;
    assert_eq!(name.as_str(), "sub");
    assert_eq!(name.to_string(), "sub");
    assert_eq!(serde_json::to_value(&name)?, json!("sub"));
    assert_eq!(serde_json::from_value::<ClaimName>(json!("sub"))?, name);
    assert_eq!(ClaimName::try_from("sub".to_string())?, name);
    assert_eq!(String::from(name), "sub");
    assert_eq!(error_of(ClaimName::new(""))?, ClaimNameError::Empty);
    assert_eq!(
        error_of(ClaimName::new(""))?.to_string(),
        "claim name must not be empty"
    );
    rejects::<ClaimName>(json!(""))
}

#[test]
fn json_key_paths_need_a_key() -> TestResult {
    let path = JsonKeyPath::new(vec!["a".to_string(), "b".to_string()])?;
    assert_eq!(path.keys(), ["a", "b"]);
    let wire = serde_json::to_value(&path)?;
    assert_eq!(wire, json!(["a", "b"]));
    assert_eq!(serde_json::from_value::<JsonKeyPath>(wire)?, path);
    assert_eq!(Vec::<String>::from(path), ["a", "b"]);
    assert_eq!(error_of(JsonKeyPath::new(vec![]))?, JsonKeyPathError::Empty);
    assert_eq!(
        error_of(JsonKeyPath::new(vec![]))?.to_string(),
        "a JSON key path must contain at least one key"
    );
    rejects::<JsonKeyPath>(json!([]))
}

#[test]
fn jwt_masking_omits_empty_fields() -> TestResult {
    assert_eq!(serde_json::to_value(JwtMasking::default())?, json!({}));
    let masking: JwtMasking =
        serde_json::from_value(json!({ "mask_claims": ["sub", "email"], "pattern": "(ey\\S+)" }))?;
    assert_eq!(
        masking.mask_claims,
        vec![ClaimName::new("sub")?, ClaimName::new("email")?]
    );
    assert_eq!(masking.pattern, Some(ExtractPattern::new("(ey\\S+)")?));
    assert_eq!(
        serde_json::to_value(&masking)?,
        json!({ "mask_claims": ["sub", "email"], "pattern": "(ey\\S+)" })
    );
    assert_eq!(serde_json::from_value::<JwtMasking>(json!({}))?, JwtMasking::default());
    rejects::<JwtMasking>(json!({ "mask_claims": [""] }))?;
    rejects::<JwtMasking>(json!({ "pattern": "none" }))
}

#[test]
fn extract_rules_are_tagged_by_variant() -> TestResult {
    let cases = [
        (
            json!({ "regex": "a=(\\w+)" }),
            Extract::Regex(ExtractPattern::new("a=(\\w+)")?),
            true,
        ),
        (
            json!({ "json_key": ["auth", "token"] }),
            Extract::JsonKey(JsonKeyPath::new(vec!["auth".to_string(), "token".to_string()])?),
            false,
        ),
        (
            json!({ "env_line": "TOKEN" }),
            Extract::EnvLine(EnvVarName::new("TOKEN")?),
            false,
        ),
        (json!({ "jwt": {} }), Extract::Jwt(JwtMasking::default()), true),
    ];
    for (wire, extract, fits_env) in cases {
        assert_eq!(serde_json::to_value(&extract)?, wire);
        assert_eq!(serde_json::from_value::<Extract>(wire.clone())?, extract);
        assert_eq!(serde_json::from_str::<Extract>(&wire.to_string())?, extract);
        assert_eq!(extract.fits_env_target(), fits_env);
    }
    rejects::<Extract>(json!({ "regex": "no group" }))?;
    rejects::<Extract>(json!({ "json_key": [] }))?;
    rejects::<Extract>(json!({ "env_line": "1" }))?;
    rejects::<Extract>(json!({ "jwt": { "mask_claims": [""] } }))?;
    rejects::<Extract>(json!({ "unknown": "x" }))
}

#[test]
fn group_names_may_spell_astral_characters_as_surrogate_pairs() -> TestResult {
    let slash = '\\';
    for pattern in [
        format!("(?<{slash}ud835{slash}udc9c>a)"),
        format!("(?<a{slash}ud835{slash}udc9c>a)"),
        format!("(a)[{slash}u0041-{slash}u005a]"),
    ] {
        assert_eq!(ExtractPattern::new(pattern.as_str())?.as_str(), pattern);
    }
    let unpaired = format!("(?<{slash}ud835{slash}u0041>a)");
    assert_eq!(
        error_of(ExtractPattern::new(unpaired.as_str()))?,
        ExtractPatternError::Invalid {
            pattern: unpaired,
            reason: RegexSyntaxError::InvalidGroupName,
        }
    );
    Ok(())
}
