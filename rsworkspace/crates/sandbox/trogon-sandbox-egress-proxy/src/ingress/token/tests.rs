use super::*;

#[test]
fn debug_never_shows_the_token() -> Result<(), Box<dyn std::error::Error>> {
    let token = ProxyToken::new("s3cr3t-value")?;
    let shown = format!("{token:?} {:#?}", Some(&token));
    assert!(!shown.contains("s3cr3t"));
    assert!(shown.contains("redacted"));
    Ok(())
}

#[test]
fn verify_accepts_only_the_exact_token() -> Result<(), Box<dyn std::error::Error>> {
    let token = ProxyToken::new("abc123")?;
    assert!(token.verify(b"abc123"));
    assert!(!token.verify(b"abc124"));
    assert!(!token.verify(b"abc12"));
    assert!(!token.verify(b"abc1234"));
    assert!(!token.verify(b""));
    assert!(!token.verify(&[b'a'; 4096]));
    Ok(())
}

#[test]
fn verify_treats_trailing_nul_as_different() -> Result<(), Box<dyn std::error::Error>> {
    let token = ProxyToken::new("abc")?;
    assert!(!token.verify(b"abc\0"));
    Ok(())
}

#[test]
fn rejects_unusable_tokens() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(ProxyToken::new(""), Err(InvalidProxyTokenError::Empty));
    assert_eq!(ProxyToken::new(&"a".repeat(256)), Err(InvalidProxyTokenError::TooLong));
    assert_eq!(ProxyToken::new("a b"), Err(InvalidProxyTokenError::ForbiddenCharacter));
    assert!(ProxyToken::new(&"a".repeat(255)).is_ok());
    Ok(())
}

#[test]
fn a_token_equals_one_holding_the_same_bytes() -> Result<(), Box<dyn std::error::Error>> {
    assert!(ProxyToken::new("abc123")? == ProxyToken::new("abc123")?);
    assert!(ProxyToken::new("abc123")? != ProxyToken::new("abc124")?);
    Ok(())
}
