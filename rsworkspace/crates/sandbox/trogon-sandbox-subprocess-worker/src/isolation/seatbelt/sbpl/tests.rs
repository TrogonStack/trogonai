use super::*;

fn tag() -> Result<LogTag, Box<dyn std::error::Error>> {
    Ok(LogTag::new("TAG")?)
}

fn string(raw: &str) -> Result<String, Box<dyn std::error::Error>> {
    Ok(SbplString::new(raw)?.to_string())
}

#[test]
fn strings_escape_quotes_backslashes_and_whitespace_controls() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(string(r#"a"b"#)?, r#""a\"b""#);
    assert_eq!(string(r"a\b")?, r#""a\\b""#);
    assert_eq!(string("a\nb\tc\rd")?, r#""a\nb\tc\rd""#);
    Ok(())
}

#[test]
fn strings_refuse_other_control_characters() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(SbplString::new("a\0b"), Err(SbplError::ControlCharacter('\0')));
    assert_eq!(SbplString::new("a\u{1b}b"), Err(SbplError::ControlCharacter('\u{1b}')));
    Ok(())
}

#[test]
fn strings_over_the_limit_are_refused_not_truncated() -> Result<(), Box<dyn std::error::Error>> {
    assert!(SbplString::new("a".repeat(MAX_STRING_BYTES)).is_ok());
    assert_eq!(
        SbplString::new("a".repeat(MAX_STRING_BYTES + 1)),
        Err(SbplError::StringTooLong {
            len: MAX_STRING_BYTES + 1,
            max: MAX_STRING_BYTES
        })
    );
    Ok(())
}

#[test]
fn a_path_with_brackets_stays_a_literal_subpath() -> Result<(), Box<dyn std::error::Error>> {
    let filter = Filter::subpath("/work/a[b/c]d")?;
    assert_eq!(filter.to_string(), r#"(subpath "/work/a[b/c]d")"#);
    Ok(())
}

#[test]
fn regex_escaping_covers_every_operator() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(escape_regex("a.b*c+d?e"), r"a\.b\*c\+d\?e");
    assert_eq!(escape_regex("^$(){}|[]\\"), r"\^\$\(\)\{\}\|\[\]\\");
    assert_eq!(escape_regex("a[b/c]d"), r"a\[b/c\]d");
    assert_eq!(escape_regex("plain/path-1_2"), "plain/path-1_2");
    Ok(())
}

#[test]
fn a_regex_filter_doubles_the_backslashes_of_its_source() -> Result<(), Box<dyn std::error::Error>> {
    let filter = Filter::regex(&format!("^{}$", escape_regex("a.b")))?;
    assert_eq!(filter.to_string(), r#"(regex "^a\\.b$")"#);
    Ok(())
}

#[test]
fn allows_render_before_denies_and_denies_carry_the_tag() -> Result<(), Box<dyn std::error::Error>> {
    let profile = ProfileBuilder::new(tag()?)
        .allow(AllowRule::new([Operation::FileReadStar], []))
        .begin_denies()
        .deny(DenyRule::new([Operation::FileReadStar], [Filter::subpath("/secret")?]))
        .build()?;
    assert_eq!(
        profile.as_str(),
        "(version 1)\n\
         (deny default (with message \"TAG\"))\n\
         (allow file-read*)\n\
         (deny file-read* (subpath \"/secret\") (with message \"TAG\"))\n"
    );
    Ok(())
}

#[test]
fn no_allow_renders_after_a_deny_whatever_the_call_order() -> Result<(), Box<dyn std::error::Error>> {
    let mut builder = ProfileBuilder::new(tag()?);
    for i in 0..5 {
        builder = builder.allow(AllowRule::new(
            [Operation::FileWriteStar],
            [Filter::subpath(&format!("/w{i}"))?],
        ));
    }
    let mut denies = builder.begin_denies();
    for i in 0..5 {
        denies = denies.deny(DenyRule::new(
            [Operation::FileWriteStar],
            [Filter::subpath(&format!("/d{i}"))?],
        ));
    }
    let text = denies.build()?.text;
    let lines: Vec<&str> = text.lines().collect();
    let last_allow = lines.iter().rposition(|l| l.starts_with("(allow")).ok_or("a value")?;
    let first_deny = lines
        .iter()
        .skip(2)
        .position(|l| l.starts_with("(deny"))
        .ok_or("a value")?
        + 2;
    assert!(last_allow < first_deny);
    Ok(())
}

#[test]
fn combinators_nest() -> Result<(), Box<dyn std::error::Error>> {
    let filter = Filter::RequireAll(vec![
        Filter::RequireAny(vec![Filter::subpath("/a")?]),
        Filter::excluding(Filter::VnodeType(VnodeType::Directory)),
    ]);
    assert_eq!(
        filter.to_string(),
        r#"(require-all (require-any (subpath "/a")) (require-not (vnode-type DIRECTORY)))"#
    );
    Ok(())
}

#[test]
fn an_oversized_profile_is_refused() -> Result<(), Box<dyn std::error::Error>> {
    let mut builder = ProfileBuilder::new(tag()?);
    for _ in 0..9000 {
        builder = builder.allow(AllowRule::new(
            [Operation::FileReadStar],
            [Filter::subpath(&"x".repeat(MAX_STRING_BYTES))?],
        ));
    }
    assert!(matches!(
        builder.begin_denies().build(),
        Err(SbplError::ProfileTooLarge { .. })
    ));
    Ok(())
}
