use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// `host.encode('idna')` of CPython 3.12.16, or the text of the `UnicodeError` it raised, for each host as code
/// points.
const PYTHON_IDNA: &str = include_str!("../tests/fixtures/sdk_mcp/python_idna.json");

fn text(code_points: &[u32]) -> PythonString {
    code_points
        .iter()
        .filter_map(|&code| PythonCodePoint::try_from(code).ok())
        .collect()
}

fn code_points(text: &str) -> Vec<u32> {
    text.chars().map(u32::from).collect()
}

#[test]
fn encode_matches_the_recorded_idna_codec() -> TestResult {
    let golden: serde_json::Value = serde_json::from_str(PYTHON_IDNA)?;
    let cases = golden["cases"].as_array().ok_or("the golden has cases")?;
    assert!(cases.len() > 3000);
    for case in cases {
        let host = case["host"]
            .as_array()
            .ok_or("each case has a host")?
            .iter()
            .map(|code| code.as_u64().and_then(|code| u32::try_from(code).ok()))
            .collect::<Option<Vec<u32>>>()
            .ok_or("each host is code points")?;
        let expected = match case["encoded"].as_str() {
            Some(encoded) => Ok(encoded.as_bytes().to_vec()),
            None => Err(case["error"]
                .as_str()
                .ok_or("a case without a name has an error")?
                .to_owned()),
        };
        assert_eq!(encode(&text(&host)), expected, "host {host:x?}");
    }
    Ok(())
}

#[test]
fn encode_keeps_an_ascii_host_and_checks_its_label_lengths() {
    assert_eq!(encode(&PythonString::from("")), Ok(Vec::new()));
    assert_eq!(
        encode(&PythonString::from("Example.COM.")),
        Ok(b"Example.COM.".to_vec())
    );
    assert_eq!(
        encode(&PythonString::from("a..b")),
        Err("label empty or too long".to_owned())
    );
    assert_eq!(
        encode(&PythonString::from("a".repeat(64).as_str())),
        Err("label too long".to_owned())
    );
}

#[test]
fn encode_splits_at_every_idna_dot_and_keeps_a_trailing_one() {
    assert_eq!(
        encode(&PythonString::from("b\u{fc}cher\u{3002}example\u{ff61}")),
        Ok(b"xn--bcher-kva.example.".to_vec())
    );
}

#[test]
fn encode_rejects_a_surrogate_as_a_prohibited_character() {
    assert_eq!(
        encode(&text(&[0x61, 0xD800])),
        Err("Invalid character '\\ud800'".to_owned())
    );
}

#[test]
fn punycode_encodes_the_rfc_3492_samples() {
    assert_eq!(punycode(&code_points("b\u{fc}cher")), b"bcher-kva".to_vec());
    assert_eq!(
        punycode(&code_points(
            "\u{4ed6}\u{4eec}\u{4e3a}\u{4ec0}\u{4e48}\u{4e0d}\u{8bf4}\u{4e2d}\u{6587}"
        )),
        b"ihqwcrb4cv8a8dqg056pqjye".to_vec()
    );
}

#[test]
fn nameprep_folds_case_and_composes_with_the_current_classes() {
    let tables = tables();
    assert_eq!(tables.nameprep(&code_points("A\u{301}")), Ok(code_points("\u{e1}")));
    assert_eq!(
        tables.nameprep(&code_points("\u{1100}\u{1161}\u{11a8}")),
        Ok(vec![0xAC01])
    );
    assert_eq!(tables.nameprep(&code_points("a\u{ad}b")), Ok(code_points("ab")));
}
