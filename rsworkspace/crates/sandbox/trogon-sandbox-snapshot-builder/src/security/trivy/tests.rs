use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn a_report_scores_its_highest_vector() -> TestResult {
    let json = br#"{
        "Results": [{
            "Vulnerabilities": [
                { "CVSS": { "first": { "V3Score": 9.8, "V2Score": 4.0 } } },
                { "CVSS": { "second": { "V2Score": 6.5 } }, "cvss_score": 2.0 }
            ]
        }]
    }"#;
    assert!((max_cvss_score(json)? - 9.8).abs() < f64::EPSILON);
    Ok(())
}

#[test]
fn a_report_without_findings_scores_zero() -> TestResult {
    assert!(max_cvss_score(br#"{ "Results": [] }"#)?.abs() < f64::EPSILON);
    Ok(())
}

#[test]
fn a_malformed_report_is_a_parse_error() {
    assert!(matches!(max_cvss_score(b"not json"), Err(TrivyError::Parse(_))));
}

#[tokio::test]
async fn a_gate_that_is_not_required_scans_nothing() -> TestResult {
    let report = scan_path(Path::new("/nonexistent"), 7.0, false).await?;
    assert!(report.is_empty());
    Ok(())
}
