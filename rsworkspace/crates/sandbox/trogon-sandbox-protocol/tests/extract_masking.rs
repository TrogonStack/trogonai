mod common;

use base64::Engine;
use base64::engine::general_purpose::{URL_SAFE, URL_SAFE_NO_PAD};
use common::{TestResult, host, hosts, name};
use secrecy::ExposeSecret;
use trogon_sandbox_contract::{
    ClaimName, EnvVarName, Extract, ExtractPattern, JsonKeyPath, JwtMasking, MaskDuplicates, SecretName,
};
use trogon_sandbox_protocol::{
    CaptureMaskError, ExtractError, ExtractKind, JwtMaskError, RealValue, Sentinel, SentinelRegistry, is_jwt,
    mask_extract, plan_extract,
};

type Boxed = Box<dyn std::error::Error>;

fn sentinel() -> Sentinel {
    Sentinel::mint(&RealValue::new("x".to_string()))
}

fn regex(raw: &str) -> Result<Extract, Boxed> {
    Ok(Extract::Regex(ExtractPattern::new(raw)?))
}

fn json_key(keys: &[&str]) -> Result<Extract, Boxed> {
    Ok(Extract::JsonKey(JsonKeyPath::new(
        keys.iter().map(|key| key.to_string()).collect(),
    )?))
}

fn env_line(variable: &str) -> Result<Extract, Boxed> {
    Ok(Extract::EnvLine(EnvVarName::new(variable)?))
}

fn jwt() -> Extract {
    Extract::Jwt(JwtMasking::default())
}

fn b64(value: &str) -> String {
    URL_SAFE_NO_PAD.encode(value)
}

fn token(payload: &str) -> String {
    format!("{}.{}.c2ln", b64(r#"{"alg":"HS256"}"#), b64(payload))
}

fn mask(rule: &Extract, content: &str, sentinel: &Sentinel) -> Result<String, ExtractError> {
    mask_extract(rule, content, sentinel)
}

fn rendered(rule: &Extract, content: &str, duplicates: MaskDuplicates) -> Result<(Vec<String>, String), Boxed> {
    let plan = plan_extract(rule, content, duplicates)?;
    let sentinels: Vec<Sentinel> = plan.captures().iter().map(|_| sentinel()).collect();
    let mut out = plan.render(&sentinels);
    for (index, minted) in sentinels.iter().enumerate() {
        out = out.replace(minted.as_str(), &format!("<S{index}>"));
    }
    Ok((plan.captures().iter().map(|capture| capture.to_string()).collect(), out))
}

#[test]
fn regex_replaces_only_capture_group_one() -> TestResult {
    let s = sentinel();
    let masked = mask(
        &regex(r"password\s+(\S+)")?,
        "machine h login u password hunter2 # keep",
        &s,
    )?;
    assert_eq!(masked, format!("machine h login u password {s} # keep"));
    Ok(())
}

#[test]
fn regex_replaces_every_match_with_the_same_sentinel() -> TestResult {
    let s = sentinel();
    assert_eq!(
        mask(&regex(r"token=(\w+)")?, "token=abc token=def", &s)?,
        format!("token={s} token={s}")
    );
    Ok(())
}

#[test]
fn regex_without_a_match_fails_closed() -> TestResult {
    assert_eq!(
        mask(&regex(r"token=(\w+)")?, "nothing here", &sentinel()),
        Err(ExtractError::NoMatch(ExtractKind::Regex))
    );
    assert_eq!(
        mask(&regex(r"token=(\w*)")?, "token=", &sentinel()),
        Err(ExtractError::NoMatch(ExtractKind::Regex))
    );
    Ok(())
}

#[test]
fn regex_whose_group_did_not_participate_is_refused() -> TestResult {
    assert_eq!(
        mask(&regex(r"token=(\w+)?")?, "token=abc token=", &sentinel()),
        Err(ExtractError::CaptureAbsent)
    );
    Ok(())
}

#[test]
fn json_key_keeps_the_rest_of_the_document_byte_for_byte() -> TestResult {
    let s = sentinel();
    let content = "{\n  \"auth\" : { \"token\":  \"real-secret\", \"user\": \"me\" },\n  \"token\": \"other\"\n}\n";
    let masked = mask(&json_key(&["auth", "token"])?, content, &s)?;
    assert_eq!(
        masked,
        format!("{{\n  \"auth\" : {{ \"token\":  \"{s}\", \"user\": \"me\" }},\n  \"token\": \"other\"\n}}\n")
    );
    serde_json::from_str::<serde_json::Value>(&masked)?;
    Ok(())
}

#[test]
fn json_key_handles_escapes_and_arrays_without_losing_the_path() -> TestResult {
    let s = sentinel();
    let content = r#"{"list":[{"token":"no"}],"a\"b":{"t":"yes \" still"},"n":1,"t":null}"#;
    let masked = mask(&json_key(&["a\"b", "t"])?, content, &s)?;
    assert_eq!(
        masked,
        format!(r#"{{"list":[{{"token":"no"}}],"a\"b":{{"t":"{s}"}},"n":1,"t":null}}"#)
    );
    Ok(())
}

#[test]
fn json_key_ignores_non_string_values_and_missing_paths() -> TestResult {
    for content in [r#"{"token": 42}"#, r#"{"other": "x"}"#, r#"{"token": ""}"#] {
        assert_eq!(
            mask(&json_key(&["token"])?, content, &sentinel()),
            Err(ExtractError::NoMatch(ExtractKind::JsonKey)),
            "{content}"
        );
    }
    Ok(())
}

#[test]
fn json_key_refuses_a_file_that_is_not_json() -> TestResult {
    assert_eq!(
        mask(&json_key(&["token"])?, "token=abc", &sentinel()),
        Err(ExtractError::NotJson)
    );
    Ok(())
}

#[test]
fn json_key_walks_nested_arrays_objects_and_literals() -> TestResult {
    let s = sentinel();
    let content = r#"{"skip":[1,-2.5e3,true,false,null,[],{}, "x", {"token":"no"}],"a":{"b":{"token":"yes"}}}"#;
    let masked = mask(&json_key(&["a", "b", "token"])?, content, &s)?;
    assert!(masked.contains(&format!(r#""token":"{s}""#)));
    assert!(masked.contains(r#"{"token":"no"}"#));
    for broken in [
        r#"{"a":"#,
        r#"{"a" 1}"#,
        r#"{"a":1,}"#,
        r#"[1 2]"#,
        r#"{"a":tru}"#,
        r#"{"a":"\u00zz"}"#,
    ] {
        assert_eq!(
            mask(&json_key(&["a"])?, broken, &sentinel()),
            Err(ExtractError::NotJson),
            "{broken}"
        );
    }
    Ok(())
}

#[test]
fn env_line_replaces_the_value_and_keeps_quotes_and_comments() -> TestResult {
    let s = sentinel();
    let content = "# comment\nOTHER=1\nexport API_KEY=\"abc def\"  # trailing\nAPI_KEY_2=zzz\n";
    assert_eq!(
        mask(&env_line("API_KEY")?, content, &s)?,
        format!("# comment\nOTHER=1\nexport API_KEY=\"{s}\"  # trailing\nAPI_KEY_2=zzz\n")
    );
    Ok(())
}

#[test]
fn env_line_handles_single_quotes_and_bare_values() -> TestResult {
    let s = sentinel();
    assert_eq!(mask(&env_line("K")?, "K='v1'\n", &s)?, format!("K='{s}'\n"));
    assert_eq!(mask(&env_line("K")?, "K = v2\n", &s)?, format!("K = {s}\n"));
    assert_eq!(mask(&env_line("K")?, "K=v3", &s)?, format!("K={s}"));
    assert_eq!(mask(&env_line("K")?, "K=v4  \r\n", &s)?, format!("K={s}  \r\n"));
    Ok(())
}

#[test]
fn env_line_without_the_variable_fails_closed() -> TestResult {
    for content in [
        "OTHER=1\n  # K=commented\nK=\n",
        "K\n",
        "KK=1\n",
        "K='\n",
        "K=''\n",
        "K=#note\n",
    ] {
        assert_eq!(
            mask(&env_line("K")?, content, &sentinel()),
            Err(ExtractError::NoMatch(ExtractKind::EnvLine)),
            "{content:?}"
        );
    }
    Ok(())
}

#[test]
fn jwt_replaces_each_verified_token_and_skips_lookalikes() -> TestResult {
    let s = sentinel();
    let (a, b) = (token(r#"{"a":"1"}"#), token(r#"{"b":"2"}"#));
    let content = format!("one={a}\nlook=eyJhbGci.notjson.sig\ntwo=\"{b}\"\n");
    assert_eq!(
        mask(&jwt(), &content, &s)?,
        format!("one={s}\nlook=eyJhbGci.notjson.sig\ntwo=\"{s}\"\n")
    );
    Ok(())
}

#[test]
fn jwt_without_a_verifiable_token_fails_closed() {
    for content in ["", "eyJhbGci.notjson.sig", "no token", "eyJ.a.b", "eyJhbGci.", "eyJ"] {
        assert_eq!(
            mask(&jwt(), content, &sentinel()),
            Err(ExtractError::NoMatch(ExtractKind::Jwt)),
            "{content}"
        );
    }
}

#[test]
fn jwt_accepts_padded_segments_and_an_empty_signature() -> TestResult {
    let s = sentinel();
    let padded = format!(
        "{}.{}.sig",
        URL_SAFE.encode(r#"{"alg":"HS256","t":10}"#),
        URL_SAFE.encode(r#"{"sub":"abc"}"#)
    );
    assert!(padded.contains("=."));
    assert_eq!(mask(&jwt(), &format!("t={padded}"), &s)?, format!("t={s}"));
    let unsigned = format!("{}.{}.", b64(r#"{"alg":"none"}"#), b64(r#"{"a":"b"}"#));
    assert_eq!(mask(&jwt(), &format!("t={unsigned}"), &s)?, format!("t={s}"));
    Ok(())
}

#[test]
fn a_jwt_rule_with_a_pattern_masks_only_the_verified_captures() -> TestResult {
    let good = format!("{}.{}.sig", b64(r#"{"alg":"HS256"}"#), b64(r#"{"sub":"a"}"#));
    let content = format!("access={good} note=not.a.jwt other={good}");
    let rule = Extract::Jwt(JwtMasking {
        mask_claims: Vec::new(),
        pattern: Some(ExtractPattern::new(r"access=(\S+)")?),
    });
    let (captures, out) = rendered(&rule, &content, MaskDuplicates::Disabled)?;
    assert_eq!(captures, vec![good.clone()]);
    assert_eq!(out, format!("access=<S0> note=not.a.jwt other={good}"));
    Ok(())
}

#[test]
fn a_jwt_rule_with_a_pattern_that_verifies_nothing_matches_nothing() -> TestResult {
    let rule = Extract::Jwt(JwtMasking {
        mask_claims: Vec::new(),
        pattern: Some(ExtractPattern::new(r"access=(\S+)")?),
    });
    let error = plan_extract(&rule, "access=plain-text", MaskDuplicates::Disabled)
        .err()
        .ok_or("expected error")?;
    assert_eq!(error, ExtractError::NoMatch(ExtractKind::Jwt));
    Ok(())
}

#[test]
fn a_verbatim_duplicate_outside_the_match_is_masked_when_asked() -> TestResult {
    let content = "oauth_token: ghp_supersecret123\n# backup copy: ghp_supersecret123\n";
    let (captures, out) = rendered(&regex(r"oauth_token:\s*(\S+)")?, content, MaskDuplicates::Enabled)?;
    assert_eq!(captures, ["ghp_supersecret123"]);
    assert_eq!(out, "oauth_token: <S0>\n# backup copy: <S0>\n");
    let (_, out) = rendered(&regex(r"oauth_token:\s*(\S+)")?, content, MaskDuplicates::Disabled)?;
    assert_eq!(out, "oauth_token: <S0>\n# backup copy: ghp_supersecret123\n");
    Ok(())
}

#[test]
fn each_distinct_capture_is_its_own_entry_in_first_seen_order() -> TestResult {
    let (captures, out) = rendered(
        &regex(r"token=(\w+)")?,
        "token=A token=B token=A",
        MaskDuplicates::Disabled,
    )?;
    assert_eq!(captures, ["A", "B"]);
    assert_eq!(out, "token=<S0> token=<S1> token=<S0>");
    Ok(())
}

#[test]
fn a_short_capture_also_replaces_coincidental_substrings() -> TestResult {
    let (_, out) = rendered(
        &regex(r"token:\s*(\S+)")?,
        "token: abc\nword: xabcy\n",
        MaskDuplicates::Enabled,
    )?;
    assert_eq!(out, "token: <S0>\nword: x<S0>y\n");
    Ok(())
}

#[test]
fn a_capture_inside_a_longer_one_cannot_take_part_of_it() -> TestResult {
    let content = "a=tok-long b=tok\n# dup: tok-long and tok\n";
    let (captures, out) = rendered(&regex(r"[ab]=(\S+)")?, content, MaskDuplicates::Enabled)?;
    assert_eq!(captures, ["tok-long", "tok"]);
    assert_eq!(out, "a=<S0> b=<S1>\n# dup: <S0> and <S1>\n");
    Ok(())
}

#[test]
fn duplicates_are_found_for_json_key_and_env_line_rules() -> TestResult {
    let (_, json) = rendered(
        &json_key(&["token"])?,
        r#"{"token":"abcdef123","note":"see abcdef123"}"#,
        MaskDuplicates::Enabled,
    )?;
    assert_eq!(json, r#"{"token":"<S0>","note":"see <S0>"}"#);
    let (_, env) = rendered(&env_line("K")?, "K=abcdef123\n# abcdef123\n", MaskDuplicates::Enabled)?;
    assert_eq!(env, "K=<S0>\n# <S0>\n");
    Ok(())
}

#[test]
fn overlapping_matches_are_left_to_the_regex_engine() -> TestResult {
    let (captures, out) = rendered(&regex("(aa)")?, "aaa", MaskDuplicates::Disabled)?;
    assert_eq!(captures, ["aa"]);
    assert_eq!(out, "<S0>a");
    let (_, out) = rendered(&regex("(aa)")?, "aaa", MaskDuplicates::Enabled)?;
    assert_eq!(out, "<S0>a");
    Ok(())
}

#[test]
fn empty_captures_are_skipped_next_to_non_empty_ones() -> TestResult {
    let (captures, out) = rendered(&regex(r"k=(\S*)")?, "k=v\nk=\n", MaskDuplicates::Disabled)?;
    assert_eq!(captures, ["v"]);
    assert_eq!(out, "k=<S0>\nk=\n");
    Ok(())
}

#[test]
fn an_unverified_jwt_lookalike_is_never_a_capture_so_its_copies_stay() -> TestResult {
    let good = token(r#"{"sub":"a"}"#);
    let content = format!("t={good}\nnote eyJhbGci.notjson.sig\n# {good}\nagain eyJhbGci.notjson.sig\n");
    let (captures, out) = rendered(&jwt(), &content, MaskDuplicates::Enabled)?;
    assert_eq!(captures, [good]);
    assert_eq!(
        out,
        "t=<S0>\nnote eyJhbGci.notjson.sig\n# <S0>\nagain eyJhbGci.notjson.sig\n"
    );
    Ok(())
}

#[test]
fn a_plan_without_a_match_fails_closed() -> TestResult {
    let error = plan_extract(&regex(r"token=(\w+)")?, "nothing", MaskDuplicates::Enabled)
        .err()
        .ok_or("expected error")?;
    assert_eq!(error, ExtractError::NoMatch(ExtractKind::Regex));
    Ok(())
}

#[test]
fn errors_never_carry_file_content() -> TestResult {
    let error = mask(&json_key(&["a"])?, "TOKEN_secret_content", &sentinel())
        .err()
        .ok_or("expected error")?;
    assert!(!format!("{error} {error:?}").contains("secret_content"));
    for kind in [
        ExtractKind::Regex,
        ExtractKind::JsonKey,
        ExtractKind::EnvLine,
        ExtractKind::Jwt,
    ] {
        assert!(!kind.to_string().is_empty());
        assert!(!ExtractError::NoMatch(kind).to_string().is_empty());
    }
    assert!(!ExtractError::CaptureAbsent.to_string().is_empty());
    Ok(())
}

fn registered(real: &str) -> Result<(SentinelRegistry, SecretName, Sentinel), Boxed> {
    let mut registry = SentinelRegistry::new();
    let secret = name("file")?;
    let sentinel = registry.register(
        secret.clone(),
        RealValue::new(real.to_string()),
        hosts(&["api.example.com"])?,
    );
    Ok((registry, secret, sentinel))
}

#[test]
fn captures_get_their_own_sentinels_that_substitute_back_per_value() -> TestResult {
    let (mut registry, secret, own) = registered("tok-one-1111")?;
    let rule = regex(r"t=(\S+)")?;
    let content = "t=tok-one-1111 t=tok-two-2222\n# tok-two-2222 and tok-one-1111\n";
    let plan = plan_extract(&rule, content, MaskDuplicates::Enabled)?;
    let sentinels = registry.register_captures(&secret, &rule, &plan, &hosts(&["api.example.com"])?)?;
    assert_eq!(sentinels.len(), 2);
    assert_eq!(sentinels[0], own);
    assert_ne!(sentinels[1], own);
    let masked = plan.render(&sentinels);
    assert!(!masked.contains("tok-one-1111") && !masked.contains("tok-two-2222"));
    let back = registry.substitute(&masked, &host("api.example.com")?, 443);
    assert_eq!(back.expose_secret(), content);
    let off = registry.substitute(&masked, &host("evil.example.com")?, 443);
    assert_eq!(off.expose_secret(), masked);
    assert_eq!(registry.len(), 2);
    assert!(registry.sentinel_for(&name("file.capture-1")?).is_some());
    Ok(())
}

#[test]
fn a_captured_jwt_gets_its_own_fake_token() -> TestResult {
    let jwt_a = token(r#"{"a":"1"}"#);
    let jwt_b = token(r#"{"b":"2"}"#);
    let (mut registry, secret, own) = registered(&jwt_a)?;
    let rule = jwt();
    let content = format!("one={jwt_a}\ntwo={jwt_b}\n");
    let plan = plan_extract(&rule, &content, MaskDuplicates::Enabled)?;
    let sentinels = registry.register_captures(&secret, &rule, &plan, &hosts(&["api.example.com"])?)?;
    assert_eq!(sentinels[0], own);
    assert!(is_jwt(sentinels[1].as_str()));
    assert_eq!(registry.lookup_real(&sentinels[1]).ok_or("real")?.expose(), jwt_b);
    Ok(())
}

#[test]
fn a_captured_jwt_with_named_claims_registers_claim_entries() -> TestResult {
    let jwt_a = token(r#"{"sub":"alice"}"#);
    let jwt_b = token(r#"{"sub":"bobby"}"#);
    let (mut registry, secret, _) = registered(&jwt_a)?;
    let rule = Extract::Jwt(JwtMasking {
        mask_claims: vec![ClaimName::new("sub")?],
        pattern: None,
    });
    let content = format!("one={jwt_a}\ntwo={jwt_b}\n");
    let plan = plan_extract(&rule, &content, MaskDuplicates::Disabled)?;
    registry.register_captures(&secret, &rule, &plan, &hosts(&["api.example.com"])?)?;
    assert!(registry.sentinel_for(&name("file.capture-1")?).is_some());
    assert!(registry.sentinel_for(&name("file.capture-1.claim-0")?).is_some());
    Ok(())
}

#[test]
#[cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    expect(
        error_string_comparison,
        reason = "the rendered message is the contract under test; typed variants are matched separately"
    )
)]
fn a_captured_jwt_that_cannot_be_masked_registers_nothing() -> TestResult {
    let jwt_a = token(r#"{"sub":"alice"}"#);
    let jwt_b = token(r#"{"other":"bobby"}"#);
    let (mut registry, secret, _) = registered(&jwt_a)?;
    let rule = Extract::Jwt(JwtMasking {
        mask_claims: vec![ClaimName::new("sub")?],
        pattern: None,
    });
    let content = format!("one={jwt_a}\ntwo={jwt_b}\n");
    let plan = plan_extract(&rule, &content, MaskDuplicates::Disabled)?;
    let error = registry
        .register_captures(&secret, &rule, &plan, &hosts(&["api.example.com"])?)
        .err()
        .ok_or("expected error")?;
    assert!(matches!(
        error,
        CaptureMaskError::Jwt {
            source: JwtMaskError::NoClaimMatched,
            ..
        }
    ));
    assert!(error.to_string().contains("file"));
    assert_eq!(registry.len(), 1);
    Ok(())
}

#[test]
fn a_capture_with_a_control_character_registers_nothing() -> TestResult {
    let (mut registry, secret, _) = registered("tok-one-1111")?;
    let rule = regex(r"t=(.+)")?;
    let plan = plan_extract(&rule, "t=tok-one-1111\nt=ok\u{1}bad", MaskDuplicates::Enabled)?;
    assert_eq!(plan.captures().len(), 2);
    assert_eq!(
        registry.register_captures(&secret, &rule, &plan, &hosts(&["api.example.com"])?),
        Err(CaptureMaskError::ControlCharacter(secret.clone()))
    );
    assert_eq!(registry.len(), 1);
    Ok(())
}

#[test]
fn captures_for_an_unknown_binding_are_refused() -> TestResult {
    let mut registry = SentinelRegistry::new();
    let rule = regex(r"t=(\S+)")?;
    let plan = plan_extract(&rule, "t=abc", MaskDuplicates::Disabled)?;
    let error = registry
        .register_captures(&name("ghost")?, &rule, &plan, &hosts(&["api.example.com"])?)
        .err()
        .ok_or("expected error")?;
    assert_eq!(error, CaptureMaskError::UnknownBinding(name("ghost")?));
    Ok(())
}

#[test]
fn named_groups_and_back_references_are_understood() -> TestResult {
    let s = sentinel();
    let masked = mask(&regex(r"(?<q>['\x22])\S+?\k<q>")?, "a 'bc' d", &s)?;
    assert_eq!(masked, format!("a {s}bc' d"));
    let masked = mask(&regex(r"(?<=key=)(\w+)")?, "key=abc", &s)?;
    assert_eq!(masked, format!("key={s}"));
    let masked = mask(&regex(r"(?<!x)key=(\w+)")?, "key=abc", &s)?;
    assert_eq!(masked, format!("key={s}"));
    Ok(())
}

#[test]
fn unicode_escapes_in_patterns_match_their_code_points() -> TestResult {
    let s = sentinel();
    let masked = mask(&regex(r"pw=(\u0061+)")?, "pw=aaa end", &s)?;
    assert_eq!(masked, format!("pw={s} end"));
    let masked = mask(&regex(r"(\ud83d\ude00)")?, "x \u{1F600} y", &s)?;
    assert_eq!(masked, format!("x {s} y"));
    let masked = mask(&regex(r"pw=(\w+)\.")?, "pw=ab.", &s)?;
    assert_eq!(masked, format!("pw={s}."));
    Ok(())
}
