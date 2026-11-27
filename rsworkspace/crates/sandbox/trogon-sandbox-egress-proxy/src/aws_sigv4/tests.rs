use super::*;

const SENTINEL_ACCESS_KEY_ID: &str = "fake_value_e5b0c442-98fc-1c14-9afb-f4c8996fb924";

fn pairs(entries: &[(&str, &str)]) -> Vec<(String, String)> {
    entries
        .iter()
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect()
}

fn scope(service: &str) -> SigningScope {
    SigningScope {
        date: "20150830".to_string(),
        region: "us-east-1".to_string(),
        service: service.to_string(),
    }
}

#[test]
fn s3_signs_the_raw_path_and_other_services_encode_it_again() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(
        canonical_uri("/bucket//key/./with%20space", "s3"),
        "/bucket//key/./with%20space"
    );
    assert_eq!(canonical_uri("", "s3"), "/");
    assert_eq!(canonical_uri("/a%20b/c", "execute-api"), "/a%2520b/c");
    assert_eq!(canonical_uri("/example/..", "service"), "/");
    assert_eq!(canonical_uri("//example//", "service"), "/example/");
    Ok(())
}

#[test]
fn the_canonical_query_sorts_and_strictly_encodes() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(canonical_query_string("b=2&a=2&a=1"), "a=1&a=2&b=2");
    assert_eq!(canonical_query_string("k=a+b"), "k=a%2Bb");
    assert_eq!(canonical_query_string("key"), "key=");
    assert_eq!(canonical_query_string("k=%zz"), "k=%25zz");
    assert_eq!(canonical_query_string("k=!'()*"), "k=%21%27%28%29%2A");
    Ok(())
}

#[test]
fn the_unsigned_payload_marker_is_signed_verbatim() -> Result<(), Box<dyn std::error::Error>> {
    let headers = pairs(&[
        ("host", "s3.amazonaws.com"),
        ("x-amz-date", "20150830T123600Z"),
        ("x-amz-content-sha256", UNSIGNED_PAYLOAD),
    ]);
    let signed_headers = SignedHeaders::new(["host", "x-amz-date", "x-amz-content-sha256"]);
    let scope = scope("s3");
    let creq = canonical_request(&SigningInput {
        method: "PUT",
        target: "/bucket/key",
        headers: &headers,
        signed_headers: &signed_headers,
        payload_hash: &PayloadHash::Unsigned,
        amz_date: "20150830T123600Z",
        scope: &scope,
    })?;

    assert!(creq.ends_with("\nUNSIGNED-PAYLOAD"));
    Ok(())
}

#[test]
fn a_missing_signed_header_is_an_error() -> Result<(), Box<dyn std::error::Error>> {
    let headers = pairs(&[("host", "example.amazonaws.com")]);
    let signed_headers = SignedHeaders::new(["host", "x-amz-date"]);
    let scope = scope("service");

    let result = canonical_request(&SigningInput {
        method: "GET",
        target: "/",
        headers: &headers,
        signed_headers: &signed_headers,
        payload_hash: &PayloadHash::of(b""),
        amz_date: "20150830T123600Z",
        scope: &scope,
    });

    assert_eq!(result, Err(MissingSignedHeaderError("x-amz-date".to_string())));
    Ok(())
}

#[test]
fn authorization_parsing_reads_scope_signed_headers_and_rejects_malformed_values()
-> Result<(), Box<dyn std::error::Error>> {
    let auth = "AWS4-HMAC-SHA256 Credential=AKIDLOCALTEST/20150830/us-east-1/service/aws4_request, SignedHeaders=host;x-amz-date, Signature=abc123def456";

    let parsed = parse_authorization(auth).ok_or("expected a value")?;

    assert_eq!(parsed.access_key_id, "AKIDLOCALTEST");
    assert_eq!(parsed.scope, scope("service"));
    assert_eq!(parsed.signed_headers, SignedHeaders::new(["host", "x-amz-date"]));
    assert!(parse_authorization("Bearer token").is_none());
    assert!(
        parse_authorization(
            "AWS4-HMAC-SHA256 Credential=AKID/20150830/us-east-1/aws4_request, SignedHeaders=host, Signature=ff"
        )
        .is_none()
    );
    assert!(parse_authorization("AWS4-HMAC-SHA256 SignedHeaders=host").is_none());
    assert!(
        parse_authorization(
            "AWS4-HMAC-SHA256 Credential=A/20150830/r/s/aws4_request, SignedHeaders=host, Signature=zz"
        )
        .is_none()
    );
    Ok(())
}

fn header_auth() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential={SENTINEL_ACCESS_KEY_ID}/20150830/us-east-1/s3/aws4_request, SignedHeaders=host;x-amz-date, Signature=ff00"
    )
}

fn presigned_target(access_key_id: &str) -> String {
    let credential = uri_encode(&format!("{access_key_id}/20150830/us-east-1/s3/aws4_request"));
    format!(
        "/key?X-Amz-Algorithm=AWS4-HMAC-SHA256&X-Amz-Credential={credential}&X-Amz-Date=20150830T123600Z&X-Amz-Expires=3600&X-Amz-SignedHeaders=host&X-Amz-Signature=ff00"
    )
}

fn detected(headers: &[(&str, &str)], target: &str) -> Option<(Sigv4Kind, String)> {
    detect(&pairs(headers), target).map(|d| (d.kind, d.access_key_id))
}

#[test]
fn header_signatures_are_classified_by_their_payload_declaration() -> Result<(), Box<dyn std::error::Error>> {
    let auth = header_auth();
    let sentinel = SENTINEL_ACCESS_KEY_ID.to_string();

    assert_eq!(
        detected(&[("Authorization", &auth)], "/"),
        Some((Sigv4Kind::Header, sentinel.clone()))
    );
    assert_eq!(
        detected(
            &[
                ("Authorization", &auth),
                ("X-Amz-Content-Sha256", &hex::encode(Sha256::digest(b"")))
            ],
            "/"
        ),
        Some((Sigv4Kind::Header, sentinel.clone()))
    );
    for marker in [
        "STREAMING-AWS4-HMAC-SHA256-PAYLOAD",
        "STREAMING-AWS4-HMAC-SHA256-PAYLOAD-TRAILER",
        "STREAMING-UNSIGNED-PAYLOAD-TRAILER",
        "STREAMING-AWS4-ECDSA-P256-SHA256-PAYLOAD",
    ] {
        assert_eq!(
            detected(
                &[
                    ("Authorization", &auth),
                    ("x-amz-content-sha256", marker),
                    ("content-encoding", "aws-chunked")
                ],
                "/key"
            ),
            Some((Sigv4Kind::Streaming, sentinel.clone())),
            "{marker}"
        );
    }
    Ok(())
}

#[test]
fn presigned_urls_and_sigv4a_are_classified() -> Result<(), Box<dyn std::error::Error>> {
    let sentinel = SENTINEL_ACCESS_KEY_ID.to_string();
    assert_eq!(
        detected(&[], &presigned_target(SENTINEL_ACCESS_KEY_ID)),
        Some((Sigv4Kind::Presigned, sentinel.clone()))
    );

    let auth = format!(
        "AWS4-ECDSA-P256-SHA256 Credential={SENTINEL_ACCESS_KEY_ID}/20150830/s3/aws4_request, SignedHeaders=host;x-amz-date;x-amz-region-set, Signature=ff00"
    );
    assert_eq!(
        detected(&[("authorization", &auth), ("x-amz-region-set", "*")], "/key"),
        Some((Sigv4Kind::Sigv4a, sentinel))
    );
    Ok(())
}

#[test]
fn requests_without_a_signature_are_not_detected() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(detected(&[("authorization", "Bearer abc")], "/"), None);
    assert_eq!(detected(&[], "/?a=1"), None);
    assert_eq!(detected(&[], "/"), None);
    assert_eq!(detected(&[], "/?X-Amz-Signature=ff00"), None);
    Ok(())
}

#[test]
fn a_junk_authorization_header_does_not_hide_a_presigned_query() -> Result<(), Box<dyn std::error::Error>> {
    let target = presigned_target(SENTINEL_ACCESS_KEY_ID);
    for authorization in [
        "Basic dXNlcjpwYXNz",
        "Bearer abc",
        "AWS4-HMAC-SHA256 garbage",
        "AWS4-ECDSA-P256-SHA256 garbage",
    ] {
        assert_eq!(
            detected(&[("authorization", authorization)], &target),
            Some((Sigv4Kind::Presigned, SENTINEL_ACCESS_KEY_ID.to_string())),
            "{authorization}"
        );
    }
    Ok(())
}

#[test]
fn a_classifiable_header_signature_wins_over_a_presigned_query() -> Result<(), Box<dyn std::error::Error>> {
    let target = presigned_target("AKIDQUERYEXAMPLE");

    assert_eq!(
        detected(&[("authorization", &header_auth())], &target),
        Some((Sigv4Kind::Header, SENTINEL_ACCESS_KEY_ID.to_string()))
    );
    Ok(())
}

#[test]
fn a_credential_with_an_empty_part_is_rejected() -> Result<(), Box<dyn std::error::Error>> {
    assert!(
        parse_authorization(
            "AWS4-HMAC-SHA256 Credential=/20150830/us-east-1/service/aws4_request, SignedHeaders=host, Signature=ff"
        )
        .is_none()
    );
    Ok(())
}

#[test]
fn a_key_longer_than_the_block_is_hashed_first() -> Result<(), Box<dyn std::error::Error>> {
    let key = [0xaau8; 131];
    let mac = hmac(&key, b"Test Using Larger Than Block-Size Key - Hash Key First");
    assert_eq!(
        hex::encode(mac),
        "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
    );
    Ok(())
}
