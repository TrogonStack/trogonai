use super::*;

fn parse(raw: &str) -> Result<SignatureInput, HttpPopError> {
    SignatureInput::parse(raw)
}

fn parse_err(raw: &str) -> HttpPopError {
    match parse(raw) {
        Ok(input) => panic!("expected {raw} to be refused, parsed {input:?}"),
        Err(err) => err,
    }
}

#[test]
fn parse_reads_components_window_and_jti() {
    let input = parse(
        r#"sig=("@method" "@authority" "@path" "signature-key");created=1730217600;expires=1730217660;nonce="jti-1""#,
    )
    .expect("parses");
    assert_eq!(input.components(), ["@method", "@authority", "@path", "signature-key"]);
    assert_eq!(input.window(), SignatureWindow::new(1_730_217_600, 1_730_217_660));
    assert_eq!(input.jti().as_str(), "jti-1");
    assert_eq!(input.alg(), None);
}

#[test]
fn parse_reads_the_declared_algorithm() {
    let input = parse(r#"sig=("@method");created=1;expires=2;nonce="n";alg="ed25519""#).expect("parses");
    assert_eq!(input.alg(), Some("ed25519"));
}

#[test]
fn parse_accepts_the_optional_whitespace_rfc_8941_allows() {
    let input = parse("\t sig=( \"@method\"  \"@path\" );  created=1; expires=2;nonce=\"n\" \t").expect("parses");
    assert_eq!(input.components(), ["@method", "@path"]);
    assert_eq!(
        input.serialize(),
        r#"("@method" "@path");created=1;expires=2;nonce="n""#
    );
}

#[test]
fn serialize_keeps_every_parameter_in_wire_order() {
    let input = parse(r#"sig=("@method" "@authority");created=1;keyid="k";expires=61;nonce="n";alg="ed25519";tag="t""#)
        .expect("parses");
    assert_eq!(
        input.serialize(),
        r#"("@method" "@authority");created=1;keyid="k";expires=61;nonce="n";alg="ed25519";tag="t""#
    );
}

#[test]
fn serialize_reescapes_strings_the_way_they_arrived() {
    let input = parse(r#"sig=("@method");created=1;expires=2;nonce="a\"b\\c""#).expect("parses");
    assert_eq!(input.jti().as_str(), r#"a"b\c"#);
    assert_eq!(input.serialize(), r#"("@method");created=1;expires=2;nonce="a\"b\\c""#);
}

#[test]
fn parse_rejects_a_second_member() {
    let err = parse_err(r#"sig=("@method");created=1;expires=2;nonce="n", other=("@path");created=1"#);
    assert!(matches!(err, HttpPopError::MalformedSignatureInput));
}

#[test]
fn parse_rejects_a_member_not_labelled_sig() {
    let err = parse_err(r#"other=("@method");created=1;expires=2;nonce="n""#);
    assert!(matches!(err, HttpPopError::MalformedSignatureInput));
}

#[test]
fn parse_rejects_a_member_key_that_cannot_start_an_http_dictionary_key() {
    let err = parse_err(r#"@sig=("@method");created=1;expires=2;nonce="n""#);
    assert!(matches!(err, HttpPopError::MalformedSignatureInput));
}

#[test]
fn parse_rejects_parameters_on_a_component() {
    let err = parse_err(r#"sig=("@query-param";name="q" "@method");created=1;expires=2;nonce="n""#);
    assert!(matches!(err, HttpPopError::MalformedSignatureInput));
}

#[test]
fn parse_rejects_a_repeated_component() {
    let err = parse_err(r#"sig=("@method" "@method");created=1;expires=2;nonce="n""#);
    assert!(matches!(err, HttpPopError::MalformedSignatureInput));
}

#[test]
fn parse_rejects_an_uppercase_component() {
    let err = parse_err(r#"sig=("Content-Digest");created=1;expires=2;nonce="n""#);
    assert!(matches!(err, HttpPopError::MalformedSignatureInput));
}

#[test]
fn parse_rejects_components_without_separating_space() {
    let err = parse_err(r#"sig=("@method""@path");created=1;expires=2;nonce="n""#);
    assert!(matches!(err, HttpPopError::MalformedSignatureInput));
}

#[test]
fn parse_rejects_an_empty_component_list() {
    let err = parse_err(r#"sig=();created=1;expires=2;nonce="n""#);
    assert!(matches!(err, HttpPopError::MalformedSignatureInput));
}

#[test]
fn parse_rejects_a_parameter_the_profile_does_not_define() {
    let err = parse_err(r#"sig=("@method");created=1;expires=2;nonce="n";extra="x""#);
    assert!(matches!(err, HttpPopError::MalformedSignatureInput));
}

#[test]
fn parse_rejects_a_repeated_parameter() {
    let err = parse_err(r#"sig=("@method");created=1;expires=2;nonce="n";created=1"#);
    assert!(matches!(err, HttpPopError::MalformedSignatureInput));
}

#[test]
fn parse_rejects_trailing_input() {
    let err = parse_err(r#"sig=("@method");created=1;expires=2;nonce="n" junk"#);
    assert!(matches!(err, HttpPopError::MalformedSignatureInput));
}

#[test]
fn parse_rejects_whitespace_before_a_parameter_separator() {
    let err = parse_err(r#"sig=("@method") ;created=1;expires=2;nonce="n""#);
    assert!(matches!(err, HttpPopError::MalformedSignatureInput));
}

#[test]
fn parse_rejects_a_created_carried_as_a_string() {
    let err = parse_err(r#"sig=("@method");created="1";expires=2;nonce="n""#);
    assert!(matches!(
        err,
        HttpPopError::InvalidSignatureParameter(SignatureParameter::Created)
    ));
}

#[test]
fn parse_rejects_a_decimal_expires() {
    let err = parse_err(r#"sig=("@method");created=1;expires=2.5;nonce="n""#);
    assert!(matches!(
        err,
        HttpPopError::InvalidSignatureParameter(SignatureParameter::Expires)
    ));
}

#[test]
fn parse_rejects_an_integer_longer_than_rfc_8941_allows() {
    let err = parse_err(r#"sig=("@method");created=1234567890123456;expires=2;nonce="n""#);
    assert!(matches!(
        err,
        HttpPopError::InvalidSignatureParameter(SignatureParameter::Created)
    ));
}

#[test]
fn parse_rejects_a_nonce_carried_as_an_integer() {
    let err = parse_err(r#"sig=("@method");created=1;expires=2;nonce=7"#);
    assert!(matches!(
        err,
        HttpPopError::InvalidSignatureParameter(SignatureParameter::Nonce)
    ));
}

#[test]
fn parse_rejects_a_boolean_parameter() {
    let err = parse_err(r#"sig=("@method");created=1;expires=2;nonce="n";alg"#);
    assert!(matches!(
        err,
        HttpPopError::InvalidSignatureParameter(SignatureParameter::Alg)
    ));
}

#[test]
fn parse_rejects_a_string_with_an_escape_rfc_8941_does_not_define() {
    let err = parse_err(r#"sig=("@method");created=1;expires=2;nonce="a\nb""#);
    assert!(matches!(
        err,
        HttpPopError::InvalidSignatureParameter(SignatureParameter::Nonce)
    ));
}

#[test]
fn parse_rejects_a_string_with_a_control_character() {
    let err = parse_err("sig=(\"@method\");created=1;expires=2;nonce=\"a\u{7f}b\"");
    assert!(matches!(
        err,
        HttpPopError::InvalidSignatureParameter(SignatureParameter::Nonce)
    ));
}

#[test]
fn parse_requires_created() {
    let err = parse_err(r#"sig=("@method");expires=2;nonce="n""#);
    assert!(matches!(
        err,
        HttpPopError::MissingSignatureParameter(SignatureParameter::Created)
    ));
}

#[test]
fn parse_requires_expires() {
    let err = parse_err(r#"sig=("@method");created=1;nonce="n""#);
    assert!(matches!(
        err,
        HttpPopError::MissingSignatureParameter(SignatureParameter::Expires)
    ));
}

#[test]
fn parse_requires_a_nonce_carrying_the_jti() {
    let err = parse_err(r#"sig=("@method");created=1;expires=2"#);
    assert!(matches!(
        err,
        HttpPopError::MissingSignatureParameter(SignatureParameter::Nonce)
    ));
}

#[test]
fn parse_rejects_an_empty_jti() {
    let err = parse_err(r#"sig=("@method");created=1;expires=2;nonce="""#);
    assert!(matches!(err, HttpPopError::InvalidJti(InvalidJtiError::Empty)));
}

#[test]
fn parse_rejects_a_jti_longer_than_a_verifier_records() {
    let jti = "a".repeat(MAX_JTI_BYTES + 1);
    let err = parse_err(&format!(r#"sig=("@method");created=1;expires=2;nonce="{jti}""#));
    assert!(matches!(
        err,
        HttpPopError::InvalidJti(InvalidJtiError::TooLong { len }) if len == MAX_JTI_BYTES + 1
    ));
}

#[test]
fn jti_accepts_up_to_the_longest_a_verifier_records() {
    let jti = "a".repeat(MAX_JTI_BYTES);
    assert_eq!(Jti::new(jti.clone()).expect("at the bound").as_str(), jti);
}

#[test]
fn signature_parameter_displays_its_wire_name() {
    assert_eq!(SignatureParameter::KeyId.to_string(), "keyid");
    assert_eq!(SignatureParameter::Created.to_string(), "created");
}
