//! Runs the signer against the AWS Signature Version 4 test suite vectors stored under `tests/fixtures`.

use std::error::Error;
use std::fs;
use std::path::Path;

use super::*;

type TestResult = Result<(), Box<dyn Error>>;

const SUITE_ACCESS_KEY_ID: &str = "AKIDEXAMPLE";
const SUITE_SECRET: &str = "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY";
const SUITE_SESSION_TOKEN: &str = "6e86291e8372ff2a2260956d9b8aae1d763fbf315fa00fa31553b73ebf194267";
const VECTOR_COUNT: usize = 22;

struct Vector {
    name: String,
    method: String,
    target: String,
    headers: Vec<(String, String)>,
    body: String,
    canonical_request: String,
    string_to_sign: String,
    authorization: String,
}

struct Request {
    method: String,
    target: String,
    headers: Vec<(String, String)>,
    body: String,
}

fn parse_request(raw: &str) -> Result<Request, Box<dyn Error>> {
    let (head, body) = raw.split_once("\n\n").unwrap_or((raw, ""));
    let mut lines = head.lines();
    let request_line = lines.next().ok_or("the request file is empty")?;
    let (method, rest) = request_line.split_once(' ').ok_or("the request line has no target")?;
    let target = rest.strip_suffix(" HTTP/1.1").unwrap_or(rest);
    let mut headers = Vec::new();
    for line in lines.filter(|line| !line.is_empty()) {
        let (name, value) = line.split_once(':').ok_or("a header line has no colon")?;
        headers.push((name.to_ascii_lowercase(), value.to_string()));
    }
    Ok(Request {
        method: method.to_string(),
        target: target.to_string(),
        headers,
        body: body.to_string(),
    })
}

fn load_vectors(dir: &Path, out: &mut Vec<Vector>) -> TestResult {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let sub = entry.path();
        let request_path = sub.join(format!("{name}.req"));
        if !request_path.exists() {
            load_vectors(&sub, out)?;
            continue;
        }
        let read = |extension: &str| -> Result<String, Box<dyn Error>> {
            Ok(fs::read_to_string(sub.join(format!("{name}.{extension}")))?
                .trim_end_matches('\n')
                .to_string())
        };
        let request = parse_request(&fs::read_to_string(&request_path)?)?;
        out.push(Vector {
            canonical_request: read("creq")?,
            string_to_sign: read("sts")?,
            authorization: read("authz")?,
            name,
            method: request.method,
            target: request.target,
            headers: request.headers,
            body: request.body,
        });
    }
    Ok(())
}

fn vectors() -> Result<Vec<Vector>, Box<dyn Error>> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/aws-sigv4-suite");
    let mut out = Vec::new();
    load_vectors(&dir, &mut out)?;
    Ok(out)
}

#[test]
fn the_signer_matches_every_vector_of_the_aws_signature_version_4_suite() -> TestResult {
    let vectors = vectors()?;
    assert_eq!(vectors.len(), VECTOR_COUNT);

    for vector in vectors {
        let mut headers = vector.headers.clone();
        if vector.name == "get-vanilla-with-session-token" {
            headers.push(("x-amz-security-token".to_string(), SUITE_SESSION_TOKEN.to_string()));
        }
        let signed_headers = SignedHeaders::new(headers.iter().map(|(name, _)| name));
        let amz_date = header_value(&headers, "x-amz-date")
            .ok_or("a vector has no x-amz-date header")?
            .to_string();
        let payload_hash = PayloadHash::of(vector.body.as_bytes());
        let scope = SigningScope {
            date: "20150830".to_string(),
            region: "us-east-1".to_string(),
            service: "service".to_string(),
        };
        let access_key_id = RealValue::new(SUITE_ACCESS_KEY_ID.to_string());
        let secret = RealValue::new(SUITE_SECRET.to_string());
        let input = SigningInput {
            method: &vector.method,
            target: &vector.target,
            headers: &headers,
            signed_headers: &signed_headers,
            payload_hash: &payload_hash,
            amz_date: &amz_date,
            scope: &scope,
        };

        let canonical_request = canonical_request(&input)?;
        assert_eq!(canonical_request, vector.canonical_request, "{}", vector.name);
        assert_eq!(
            string_to_sign(&canonical_request, &amz_date, &scope),
            vector.string_to_sign,
            "{}",
            vector.name
        );
        assert_eq!(
            sign(&input, &access_key_id, &secret)?,
            vector.authorization,
            "{}",
            vector.name
        );

        let reordered = SignedHeaders::new(headers.iter().rev().map(|(name, _)| name.to_ascii_uppercase()));
        let again = sign(
            &SigningInput {
                signed_headers: &reordered,
                ..input
            },
            &access_key_id,
            &secret,
        )?;
        assert_eq!(again, vector.authorization, "{} reordered", vector.name);
    }
    Ok(())
}
