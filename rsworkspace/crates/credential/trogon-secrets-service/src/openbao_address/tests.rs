use super::*;

#[test]
fn an_openbao_address_that_is_not_a_url_is_refused() {
    let refused = OpenBaoAddress::new("openbao.internal", None).unwrap_err();

    assert_eq!(
        refused,
        OpenBaoAddressError::NotAUrl(url::ParseError::RelativeUrlWithoutBase)
    );
}

#[test]
fn an_openbao_address_carrying_credentials_is_refused() {
    for address in [
        "https://fake-user@openbao.internal:8200",
        "https://:fake-password@openbao.internal:8200",
    ] {
        let refused = OpenBaoAddress::new(address, None).unwrap_err();

        assert_eq!(refused, OpenBaoAddressError::Credentials);
    }
}

#[test]
fn an_openbao_address_of_another_scheme_is_refused() {
    let refused = OpenBaoAddress::new("ftp://openbao.internal", None).unwrap_err();

    assert_eq!(refused, OpenBaoAddressError::UnsupportedScheme("ftp".to_owned()));
}

#[test]
fn a_plaintext_openbao_address_without_tls_disabled_is_refused() {
    let refused = OpenBaoAddress::new("http://openbao.internal:8200", None).unwrap_err();

    assert_eq!(refused, OpenBaoAddressError::Plaintext);
}

#[test]
fn an_https_openbao_address_with_tls_disabled_is_refused() {
    let refused = OpenBaoAddress::new("https://openbao.internal:8200", Some("true")).unwrap_err();

    assert_eq!(refused, OpenBaoAddressError::TlsDisabledForHttps);
}

#[test]
fn a_tls_switch_holding_an_unknown_value_is_refused() {
    let refused = OpenBaoAddress::new("http://openbao.internal:8200", Some("false")).unwrap_err();

    assert_eq!(
        refused,
        OpenBaoAddressError::UnrecognizedTlsDisabledValue {
            value: "false".to_owned()
        }
    );
}

#[test]
fn an_https_openbao_address_requires_tls() {
    let address = OpenBaoAddress::new("https://openbao.internal:8200", None).unwrap();

    assert_eq!(address.as_str(), "https://openbao.internal:8200/");
    assert_eq!(address.transport(), OpenBaoTransport::TlsRequired);
}

#[test]
fn a_plaintext_openbao_address_is_allowed_when_tls_is_disabled() {
    for switch in ["true", "1", "yes", "TRUE", "Yes"] {
        let address = OpenBaoAddress::new("http://127.0.0.1:8200", Some(switch)).unwrap();

        assert_eq!(address.transport(), OpenBaoTransport::PlaintextAllowed, "{switch}");
    }
}

#[test]
fn an_empty_tls_switch_counts_as_unset() {
    let refused = OpenBaoAddress::new("http://openbao.internal:8200", Some("")).unwrap_err();

    assert_eq!(refused, OpenBaoAddressError::Plaintext);
    assert_eq!(
        OpenBaoAddress::new("https://openbao.internal:8200", Some(""))
            .unwrap()
            .transport(),
        OpenBaoTransport::TlsRequired
    );
}

#[test]
#[cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    expect(
        error_string_comparison,
        reason = "naming the switch is the contract under test; typed variants separately prove the classification"
    )
)]
fn a_refusal_that_turns_on_the_tls_switch_names_it() {
    for refused in [
        OpenBaoAddressError::Plaintext,
        OpenBaoAddressError::TlsDisabledForHttps,
        OpenBaoAddressError::UnrecognizedTlsDisabledValue {
            value: "false".to_owned(),
        },
    ] {
        assert!(refused.to_string().contains("OPENBAO_TLS_DISABLED"), "{refused}");
    }
}
