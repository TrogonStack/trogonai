use serde::de::DeserializeOwned;
use serde_json::error::Category::Data;
use serde_json::json;
use trogon_sandbox_contract::{
    DomainPattern, HeaderCondition, HeaderName, HeaderNameError, HeaderValue, HeaderValueError, HttpMethod,
    HttpMethodError, PathPrefix, PathPrefixError, RequestRule, RequestRuleAction,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn error_of<T, E>(result: Result<T, E>) -> Result<E, Box<dyn std::error::Error>> {
    result.err().ok_or_else(|| "expected an error".into())
}

fn rejects<T: DeserializeOwned>(value: serde_json::Value) -> TestResult {
    let from_value = error_of(serde_json::from_value::<T>(value.clone()))?;
    assert_eq!(from_value.classify(), Data);
    let from_str = error_of(serde_json::from_str::<T>(&value.to_string()))?;
    assert_eq!(from_str.classify(), Data);
    Ok(())
}

#[test]
fn an_http_method_is_a_bounded_uppercase_token() -> TestResult {
    let method = HttpMethod::new("patch")?;
    assert_eq!(method.as_str(), "PATCH");
    assert_eq!(method.to_string(), "PATCH");
    assert_eq!("m-search".parse::<HttpMethod>()?.as_str(), "M-SEARCH");
    assert_eq!(HttpMethod::try_from("get".to_string())?.as_str(), "GET");
    assert_eq!(String::from(method), "PATCH");
    assert_eq!(HttpMethod::MAX_LEN, 32);
    assert_eq!(HttpMethod::new("A".repeat(HttpMethod::MAX_LEN))?.as_str().len(), 32);

    assert_eq!(error_of(HttpMethod::new(""))?, HttpMethodError::Empty);
    assert_eq!(
        error_of(HttpMethod::new("A".repeat(HttpMethod::MAX_LEN + 1)))?,
        HttpMethodError::TooLong { max: 32 }
    );
    for input in ["GE T", "GET\n", "G(T", "G\u{e9}T"] {
        assert_eq!(
            error_of(HttpMethod::new(input))?,
            HttpMethodError::NotAToken,
            "{input:?}"
        );
    }
    Ok(())
}

#[test]
fn an_http_method_serializes_as_an_uppercase_string() -> TestResult {
    let method = HttpMethod::new("get")?;
    assert_eq!(serde_json::to_value(&method)?, json!("GET"));
    assert_eq!(serde_json::from_value::<HttpMethod>(json!("get"))?, method);
    assert_eq!(serde_json::from_str::<HttpMethod>("\"get\"")?, method);
    rejects::<HttpMethod>(json!(""))?;
    rejects::<HttpMethod>(json!("GE T"))?;
    Ok(())
}

#[test]
fn a_header_name_is_a_bounded_lowercase_token() -> TestResult {
    let name = HeaderName::new("X-Trace-Id")?;
    assert_eq!(name.as_str(), "x-trace-id");
    assert_eq!(name.to_string(), "x-trace-id");
    assert_eq!("Accept".parse::<HeaderName>()?.as_str(), "accept");
    assert_eq!(HeaderName::try_from("Host".to_string())?.as_str(), "host");
    assert_eq!(String::from(name), "x-trace-id");
    assert_eq!(HeaderName::MAX_LEN, 256);
    assert_eq!(HeaderName::new("a".repeat(HeaderName::MAX_LEN))?.as_str().len(), 256);

    assert_eq!(error_of(HeaderName::new(""))?, HeaderNameError::Empty);
    assert_eq!(
        error_of(HeaderName::new("a".repeat(HeaderName::MAX_LEN + 1)))?,
        HeaderNameError::TooLong { max: 256 }
    );
    for input in ["x trace", "x:trace", "x-trace\r\n", "x-\u{e9}"] {
        assert_eq!(
            error_of(HeaderName::new(input))?,
            HeaderNameError::NotAToken,
            "{input:?}"
        );
    }
    Ok(())
}

#[test]
fn a_header_name_serializes_as_a_lowercase_string() -> TestResult {
    let name = HeaderName::new("X-Trace")?;
    assert_eq!(serde_json::to_value(&name)?, json!("x-trace"));
    assert_eq!(serde_json::from_value::<HeaderName>(json!("X-TRACE"))?, name);
    assert_eq!(serde_json::from_str::<HeaderName>("\"X-TRACE\"")?, name);
    rejects::<HeaderName>(json!(""))?;
    rejects::<HeaderName>(json!("x trace"))?;
    Ok(())
}

#[test]
fn a_header_value_is_trimmed_and_free_of_control_characters() -> TestResult {
    let value = HeaderValue::new(" \ton\t ")?;
    assert_eq!(value.as_str(), "on");
    assert_eq!(value.to_string(), "on");
    assert_eq!(HeaderValue::new("a\tb")?.as_str(), "a\tb");
    assert_eq!("Bearer x".parse::<HeaderValue>()?.as_str(), "Bearer x");
    assert_eq!(HeaderValue::try_from(" x ".to_string())?.as_str(), "x");
    assert_eq!(String::from(value), "on");
    assert_eq!(HeaderValue::MAX_LEN, 8192);
    assert_eq!(HeaderValue::new("a".repeat(HeaderValue::MAX_LEN))?.as_str().len(), 8192);

    for input in ["", " ", "\t \t"] {
        assert_eq!(error_of(HeaderValue::new(input))?, HeaderValueError::Empty, "{input:?}");
    }
    assert_eq!(
        error_of(HeaderValue::new("a".repeat(HeaderValue::MAX_LEN + 1)))?,
        HeaderValueError::TooLong { max: 8192 }
    );
    for input in ["a\nb", "a\u{0}b", "a\u{7f}b", "a\rb"] {
        assert_eq!(
            error_of(HeaderValue::new(input))?,
            HeaderValueError::ControlCharacter,
            "{input:?}"
        );
    }
    Ok(())
}

#[test]
fn a_header_value_serializes_as_a_trimmed_string() -> TestResult {
    let value = HeaderValue::new("on")?;
    assert_eq!(serde_json::to_value(&value)?, json!("on"));
    assert_eq!(serde_json::from_value::<HeaderValue>(json!(" on "))?, value);
    assert_eq!(serde_json::from_str::<HeaderValue>("\" on \"")?, value);
    rejects::<HeaderValue>(json!(""))?;
    rejects::<HeaderValue>(json!("a\nb"))?;
    Ok(())
}

#[test]
fn a_path_prefix_is_absolute_and_unambiguous() -> TestResult {
    for (input, canonical) in [
        ("/", "/"),
        ("/v1", "/v1"),
        ("/v1/", "/v1"),
        ("/api/admin", "/api/admin"),
        ("/a-b_c.d~e", "/a-b_c.d~e"),
    ] {
        let prefix = PathPrefix::new(input)?;
        assert_eq!(prefix.as_str(), canonical);
        assert_eq!(prefix.to_string(), canonical);
        assert_eq!(input.parse::<PathPrefix>()?, prefix);
        assert_eq!(PathPrefix::try_from(input.to_string())?, prefix);
        assert_eq!(String::from(prefix), canonical);
    }
    assert_eq!(PathPrefix::new("/v1/")?, PathPrefix::new("/v1")?);
    assert_eq!(PathPrefix::MAX_LEN, 2048);
    Ok(())
}

#[test]
fn a_path_prefix_rejects_what_a_server_could_read_differently() -> TestResult {
    for input in ["", "v1", "x/v1"] {
        assert_eq!(
            error_of(PathPrefix::new(input))?,
            PathPrefixError::NotAbsolute,
            "{input:?}"
        );
    }
    let long = format!("/{}", "a".repeat(PathPrefix::MAX_LEN));
    assert_eq!(
        error_of(long.parse::<PathPrefix>())?,
        PathPrefixError::TooLong { max: 2048 }
    );
    assert_eq!(error_of(PathPrefix::new(long))?, PathPrefixError::TooLong { max: 2048 });
    assert_eq!(
        error_of(PathPrefix::new("/a b".to_string()))?,
        PathPrefixError::Forbidden(' ')
    );
    for (input, bad) in [
        ("/a b", ' '),
        ("/a?b", '?'),
        ("/a#b", '#'),
        ("/a%2Fb", '%'),
        ("/a\\b", '\\'),
        ("/caf\u{e9}", '\u{e9}'),
    ] {
        assert_eq!(
            error_of(PathPrefix::new(input))?,
            PathPrefixError::Forbidden(bad),
            "{input:?}"
        );
    }
    for input in ["/a/../b", "/./a", "//a", "/a//b", "/a/.", "/.."] {
        assert_eq!(
            error_of(PathPrefix::new(input))?,
            PathPrefixError::Ambiguous,
            "{input:?}"
        );
    }
    Ok(())
}

#[test]
fn a_path_prefix_serializes_as_a_string() -> TestResult {
    let prefix = PathPrefix::new("/v1/")?;
    assert_eq!(serde_json::to_value(&prefix)?, json!("/v1"));
    assert_eq!(serde_json::from_value::<PathPrefix>(json!("/v1/"))?, prefix);
    assert_eq!(serde_json::from_str::<PathPrefix>("\"/v1/\"")?, prefix);
    rejects::<PathPrefix>(json!("x"))?;
    rejects::<PathPrefix>(json!("/a/../b"))?;
    Ok(())
}

#[test]
fn every_request_rule_error_has_a_message() {
    let cases = [
        (HttpMethodError::Empty.to_string(), "HTTP method must not be empty"),
        (
            HttpMethodError::TooLong { max: 32 }.to_string(),
            "HTTP method must be at most 32 bytes",
        ),
        (
            HttpMethodError::NotAToken.to_string(),
            "HTTP method must be a token of letters, digits, and the characters !#$%&'*+-.^_`|~",
        ),
        (HeaderNameError::Empty.to_string(), "HTTP header name must not be empty"),
        (
            HeaderNameError::TooLong { max: 256 }.to_string(),
            "HTTP header name must be at most 256 bytes",
        ),
        (
            HeaderNameError::NotAToken.to_string(),
            "HTTP header name must be a token of letters, digits, and the characters !#$%&'*+-.^_`|~",
        ),
        (
            HeaderValueError::Empty.to_string(),
            "HTTP header value must not be empty",
        ),
        (
            HeaderValueError::TooLong { max: 8192 }.to_string(),
            "HTTP header value must be at most 8192 bytes",
        ),
        (
            HeaderValueError::ControlCharacter.to_string(),
            "HTTP header value must not contain control characters",
        ),
        (
            PathPrefixError::NotAbsolute.to_string(),
            "a path prefix starts with '/'",
        ),
        (
            PathPrefixError::TooLong { max: 2048 }.to_string(),
            "a path prefix is at most 2048 bytes",
        ),
        (
            PathPrefixError::Forbidden('?').to_string(),
            "'?' is not allowed in a path prefix",
        ),
        (
            PathPrefixError::Ambiguous.to_string(),
            "a path prefix has no dot segments or empty segments",
        ),
    ];
    for (actual, expected) in cases {
        assert_eq!(actual, expected);
    }
}

#[test]
fn a_header_condition_names_a_header_with_an_optional_value() -> TestResult {
    let present = HeaderCondition {
        name: HeaderName::new("X-Trace")?,
        value: None,
    };
    assert_eq!(serde_json::to_value(&present)?, json!({"name": "x-trace"}));
    assert_eq!(
        serde_json::from_value::<HeaderCondition>(json!({"name": "X-Trace"}))?,
        present
    );

    let valued = HeaderCondition {
        name: HeaderName::new("x-role")?,
        value: Some(HeaderValue::new("root")?),
    };
    assert_eq!(
        serde_json::to_value(&valued)?,
        json!({"name": "x-role", "value": "root"})
    );
    assert_eq!(
        serde_json::from_value::<HeaderCondition>(json!({"name": "x-role", "value": "root"}))?,
        valued
    );
    Ok(())
}

#[test]
fn a_request_rule_round_trips_and_omits_empty_conditions() -> TestResult {
    let minimal = RequestRule {
        action: RequestRuleAction::Deny,
        domains: Vec::new(),
        methods: Vec::new(),
        path_prefix: None,
        headers: Vec::new(),
    };
    assert_eq!(serde_json::to_value(&minimal)?, json!({"action": "deny"}));
    assert_eq!(
        serde_json::from_value::<RequestRule>(json!({"action": "deny"}))?,
        minimal
    );

    let full = RequestRule {
        action: RequestRuleAction::Allow,
        domains: vec![DomainPattern::new("api.example.com:443")?],
        methods: vec![HttpMethod::new("get")?],
        path_prefix: Some(PathPrefix::new("/v1")?),
        headers: vec![HeaderCondition {
            name: HeaderName::new("x-trace")?,
            value: None,
        }],
    };
    let wire = json!({
        "action": "allow",
        "domains": ["api.example.com:443"],
        "methods": ["GET"],
        "path_prefix": "/v1",
        "headers": [{"name": "x-trace"}],
    });
    assert_eq!(serde_json::to_value(&full)?, wire);
    assert_eq!(serde_json::from_value::<RequestRule>(wire)?, full);
    Ok(())
}

#[test]
fn a_request_rule_rejects_a_bad_condition() -> TestResult {
    rejects::<RequestRule>(json!({"action": "deny", "path_prefix": "x"}))?;
    rejects::<RequestRule>(json!({"action": "deny", "methods": ["GE T"]}))?;
    rejects::<RequestRule>(json!({"action": "deny", "domains": ["*"]}))?;
    rejects::<RequestRule>(json!({"action": "maybe"}))?;
    Ok(())
}
