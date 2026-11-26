use trogon_sandbox_contract::{
    NoProxy, NoProxyCidr, NoProxyCidrError, NoProxyEntry, ParentProxy, ParentProxyError, ParentProxyUrl,
    ParentProxyUrlError, Port, ProxyScheme,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn error_of<T, E>(result: Result<T, E>) -> Result<E, Box<dyn std::error::Error>> {
    result.err().ok_or_else(|| "expected an error".into())
}

fn cidr(value: &str) -> Result<NoProxyEntry, Box<dyn std::error::Error>> {
    Ok(NoProxyEntry::Cidr(NoProxyCidr::new(value)?))
}

fn suffix(value: &str) -> NoProxyEntry {
    NoProxyEntry::Suffix(value.to_string())
}

#[test]
fn a_parent_proxy_url_reads_scheme_host_and_port() -> TestResult {
    let url = ParentProxyUrl::new("http://proxy.example.com:8080")?;
    assert_eq!(url.scheme(), ProxyScheme::Http);
    assert_eq!(url.host().to_string(), "proxy.example.com");
    assert_eq!(url.port(), Port::new(8080)?);
    assert_eq!(url.authorization(), None);

    let secure = ParentProxyUrl::new("https://proxy.example.com")?;
    assert_eq!(secure.scheme(), ProxyScheme::Https);
    assert_eq!(secure.port(), Port::new(443)?);

    let schemeless = ParentProxyUrl::new(" proxy.example.com:3128 ")?;
    assert_eq!(schemeless.scheme(), ProxyScheme::Http);
    assert_eq!(schemeless.port(), Port::new(3128)?);

    let ipv6 = ParentProxyUrl::new("http://[::1]:8080")?;
    assert_eq!(ipv6.host().to_string(), "[::1]");
    Ok(())
}

#[test]
fn parent_proxy_userinfo_becomes_a_basic_authorization() -> TestResult {
    let url = ParentProxyUrl::new("http://user:p%40ss@proxy.example.com:8080")?;
    assert_eq!(url.authorization(), Some("Basic dXNlcjpwQHNz"));

    assert_eq!(
        ParentProxyUrl::new("http://a+b:c%26d@h.example.com")?.authorization(),
        Some("Basic YStiOmMmZA==")
    );
    assert_eq!(
        ParentProxyUrl::new("http://user@h.example.com")?.authorization(),
        Some("Basic dXNlcjo=")
    );
    assert_eq!(
        ParentProxyUrl::new("http://:pw@h.example.com")?.authorization(),
        Some("Basic OnB3")
    );
    assert_eq!(
        ParentProxyUrl::new("http://%ff:x@h.example.com")?.authorization(),
        Some("Basic 77+9Ong=")
    );
    assert_eq!(
        ParentProxyUrl::new("http://u%3Dv:x@h.example.com")?.authorization(),
        Some("Basic dT12Ong=")
    );
    Ok(())
}

#[test]
fn a_parent_proxy_url_never_prints_its_credentials() -> TestResult {
    let with_secret = ParentProxyUrl::new("http://user:hunter2@proxy.example.com:8080")?;
    let printed = format!("{with_secret:?}");
    assert_eq!(
        printed,
        format!(
            "ParentProxyUrl {{ scheme: Http, host: {:?}, port: {:?}, authorization: Some(\"[REDACTED]\") }}",
            with_secret.host(),
            with_secret.port()
        )
    );
    assert!(!printed.contains("hunter2"));

    let without = ParentProxyUrl::new("http://proxy.example.com:8080")?;
    assert_eq!(
        format!("{without:?}"),
        format!(
            "ParentProxyUrl {{ scheme: Http, host: {:?}, port: {:?}, authorization: None }}",
            without.host(),
            without.port()
        )
    );
    Ok(())
}

#[test]
fn a_parent_proxy_url_rejects_what_is_no_proxy() -> TestResult {
    let scheme = error_of(ParentProxyUrl::new("ftp://proxy.example.com"))?;
    assert_eq!(scheme, ParentProxyUrlError::Scheme);
    assert_eq!(scheme.to_string(), "a proxy URL must use http or https");

    let leaked = error_of(ParentProxyUrl::new("http://user:hunter2@"))?;
    assert_eq!(leaked, ParentProxyUrlError::Invalid("http://***:***@".to_string()));
    assert_eq!(leaked.to_string(), "\"http://***:***@\" is not a proxy URL");

    assert_eq!(
        error_of(ParentProxyUrl::new("http://proxy.example.com:99999"))?,
        ParentProxyUrlError::Invalid("http://proxy.example.com:99999".to_string())
    );
    assert_eq!(
        error_of(ParentProxyUrl::new("http://proxy.example.com:0"))?,
        ParentProxyUrlError::Invalid("http://proxy.example.com:0".to_string())
    );
    assert_eq!(
        error_of(ParentProxyUrl::new("bad host"))?,
        ParentProxyUrlError::Invalid("bad host".to_string())
    );
    assert_eq!(
        error_of(ParentProxyUrl::new("http:///@"))?,
        ParentProxyUrlError::Invalid("http:///@".to_string())
    );
    let long_label = format!("http://{}.example.com", "a".repeat(64));
    assert_eq!(
        error_of(ParentProxyUrl::new(&long_label))?,
        ParentProxyUrlError::Invalid(long_label)
    );
    Ok(())
}

#[test]
fn a_no_proxy_list_reads_wildcards_names_and_ranges() -> TestResult {
    assert!(NoProxy::default().is_empty());
    assert!(NoProxy::new("").is_empty());
    assert!(NoProxy::new(" , ,").is_empty());
    assert_eq!(NoProxy::new("*").entries(), [NoProxyEntry::Wildcard]);

    let list = NoProxy::new(".corp.example, *.Example.org ,Host.Name:8080,plain.example,host:");
    assert!(!list.is_empty());
    assert_eq!(
        list.entries(),
        [
            suffix(".corp.example"),
            suffix(".example.org"),
            suffix("host.name"),
            suffix("plain.example"),
            suffix("host:"),
        ]
    );

    let ranges = NoProxy::new("10.0.0.0/8,192.168.1.5,[::1],[::1]:8080,fe80::1%eth0,fe80::1%eth0/64");
    assert_eq!(
        ranges.entries(),
        [
            cidr("10.0.0.0/8")?,
            cidr("192.168.1.5")?,
            cidr("::1")?,
            cidr("::1")?,
            cidr("fe80::1")?,
            cidr("fe80::1/64")?,
        ]
    );
    Ok(())
}

#[test]
fn a_no_proxy_list_ignores_malformed_ranges_and_keeps_odd_names() -> TestResult {
    let ignored = NoProxy::new("10.0.0.0/99,bad/8,1.2.3.4/x");
    assert!(ignored.is_empty());

    let odd = NoProxy::new("[::1]x,[::1,[::1]:,name%x");
    assert_eq!(
        odd.entries(),
        [suffix("[::1]x"), suffix("[:"), suffix("[::1]:"), suffix("name%x")]
    );
    Ok(())
}

#[test]
fn a_no_proxy_cidr_normalizes_ipv4_mapped_networks() -> TestResult {
    let mapped = NoProxyCidr::new("::ffff:1.2.3.4/120")?;
    assert_eq!(mapped.network().to_string(), "1.2.3.4");
    assert_eq!(mapped.prefix(), 24);

    let single = NoProxyCidr::new("::ffff:1.2.3.4")?;
    assert_eq!(single.network().to_string(), "1.2.3.4");
    assert_eq!(single.prefix(), 32);

    let wide = NoProxyCidr::new("::ffff:1.2.3.4/64")?;
    assert_eq!(wide.network().to_string(), "::ffff:1.2.3.4");
    assert_eq!(wide.prefix(), 64);

    assert_eq!(NoProxyCidr::new("2001:db8::")?.prefix(), 128);
    assert_eq!(NoProxyCidr::from("::ffff:1.2.3.4".parse::<std::net::IpAddr>()?), single);
    assert_eq!(NoProxyCidr::from("10.1.2.3".parse::<std::net::IpAddr>()?).prefix(), 32);
    Ok(())
}

#[test]
fn a_no_proxy_cidr_rejects_what_is_no_range() -> TestResult {
    let invalid = error_of(NoProxyCidr::new("nope"))?;
    assert_eq!(invalid, NoProxyCidrError::Invalid("nope".to_string()));
    assert_eq!(
        invalid.to_string(),
        "'nope' is not an IP address with an optional prefix length"
    );
    assert_eq!(
        error_of(NoProxyCidr::new("10.0.0.0/x"))?,
        NoProxyCidrError::Invalid("10.0.0.0/x".to_string())
    );

    let long_v4 = error_of(NoProxyCidr::new("1.2.3.4/33"))?;
    assert_eq!(
        long_v4,
        NoProxyCidrError::PrefixTooLong {
            input: "1.2.3.4/33".to_string(),
            max: 32
        }
    );
    assert_eq!(long_v4.to_string(), "the prefix length of '1.2.3.4/33' is above 32");
    assert_eq!(
        error_of(NoProxyCidr::new("::1/129"))?,
        NoProxyCidrError::PrefixTooLong {
            input: "::1/129".to_string(),
            max: 128
        }
    );
    Ok(())
}

#[test]
fn a_parent_proxy_needs_at_least_one_url() -> TestResult {
    let http = ParentProxyUrl::new("http://proxy.example.com:8080")?;
    let https = ParentProxyUrl::new("https://secure.example.com")?;
    let no_proxy = NoProxy::new("*.internal.example");

    let both = ParentProxy::new(Some(http.clone()), Some(https.clone()), no_proxy.clone())?;
    assert_eq!(both.http(), Some(&http));
    assert_eq!(both.https(), Some(&https));
    assert_eq!(both.no_proxy(), &no_proxy);

    let http_only = ParentProxy::new(Some(http.clone()), None, NoProxy::default())?;
    assert_eq!(http_only.https(), None);
    let https_only = ParentProxy::new(None, Some(https), NoProxy::default())?;
    assert_eq!(https_only.http(), None);

    let error = error_of(ParentProxy::new(None, None, no_proxy))?;
    assert_eq!(error, ParentProxyError::NoProxyUrl);
    assert_eq!(
        error.to_string(),
        "a parent proxy needs an http proxy URL, an https proxy URL, or both"
    );
    Ok(())
}
