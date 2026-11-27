use super::*;
use trogon_sandbox_contract::{
    DomainPattern, InjectHosts, InjectMethod, InjectMethodSet, InjectPathPrefix, InjectScope, InjectScopeVersion,
};

type TestResult<T> = Result<T, Box<dyn std::error::Error>>;

pub(crate) const API_REAL: &str = "ghp_realApiSecretValue0123456789";
pub(crate) const REGISTRY_REAL: &str = "npm_realRegistrySecretValue";

pub(crate) struct Fixture {
    pub(crate) substitution: SecretSubstitution,
    pub(crate) api: String,
    pub(crate) registry: String,
}

fn hosts(host: &str) -> TestResult<InjectHosts> {
    Ok(InjectHosts::new(vec![DomainPattern::new(host)?])?)
}

pub(crate) fn fixture() -> TestResult<Fixture> {
    let mut registry = SentinelRegistry::new();
    let api = registry.register(
        SecretName::new("api")?,
        RealValue::new(API_REAL.to_string()),
        hosts("api.example.com")?,
    );
    let package_registry = registry.register(
        SecretName::new("registry")?,
        RealValue::new(REGISTRY_REAL.to_string()),
        hosts("registry.example.org")?,
    );
    Ok(Fixture {
        substitution: SecretSubstitution::new(Arc::new(registry)),
        api: api.to_string(),
        registry: package_registry.to_string(),
    })
}

pub(crate) fn scope(fixture: &Fixture, host: &str) -> TestResult<SubstitutionScope> {
    Ok(fixture
        .substitution
        .scope_for(&TlsTarget::new(host, 443))
        .ok_or("expected a scope")?)
}

fn run(scope: &SubstitutionScope, chunks: &[&[u8]]) -> (Vec<u8>, BTreeSet<SecretName>) {
    let mut scanner = scope.scanner();
    let mut out = Vec::new();
    for chunk in chunks {
        scanner.feed(chunk, &mut out);
    }
    scanner.finish(&mut out);
    (out, scanner.take_flagged())
}

#[test]
fn replaces_a_sentinel_whose_binding_injects_at_the_target() -> Result<(), Box<dyn std::error::Error>> {
    let fx = fixture()?;
    let scope = scope(&fx, "api.example.com")?;
    let body = format!("token={}&x=1", fx.api);

    let (out, flagged) = run(&scope, &[body.as_bytes()]);

    assert_eq!(out, format!("token={API_REAL}&x=1").into_bytes());
    assert!(flagged.is_empty());
    Ok(())
}

#[test]
fn leaves_another_bindings_sentinel_and_flags_it_by_name() -> Result<(), Box<dyn std::error::Error>> {
    let fx = fixture()?;
    let scope = scope(&fx, "api.example.com")?;
    let body = format!("a={} b={}", fx.api, fx.registry);

    let (out, flagged) = run(&scope, &[body.as_bytes()]);

    assert_eq!(out, format!("a={API_REAL} b={}", fx.registry).into_bytes());
    assert_eq!(
        flagged.iter().map(SecretName::as_str).collect::<Vec<_>>(),
        vec!["registry"]
    );
    Ok(())
}

#[test]
fn a_sentinel_split_at_every_boundary_is_still_replaced() -> Result<(), Box<dyn std::error::Error>> {
    let fx = fixture()?;
    let scope = scope(&fx, "api.example.com")?;
    let body = format!("xx{}yy{}zz", fx.api, fx.api).into_bytes();
    let expected = format!("xx{API_REAL}yy{API_REAL}zz").into_bytes();

    for split in 0..=body.len() {
        let (out, _) = run(&scope, &[&body[..split], &body[split..]]);
        assert_eq!(out, expected, "split at {split}");
    }
    Ok(())
}

#[test]
fn a_sentinel_fed_one_byte_at_a_time_is_replaced() -> Result<(), Box<dyn std::error::Error>> {
    let fx = fixture()?;
    let scope = scope(&fx, "api.example.com")?;
    let body = format!("[{}]", fx.api).into_bytes();
    let chunks: Vec<&[u8]> = body.chunks(1).collect();

    let (out, _) = run(&scope, &chunks);

    assert_eq!(out, format!("[{API_REAL}]").into_bytes());
    Ok(())
}

#[test]
fn a_truncated_sentinel_prefix_is_flushed_verbatim() -> Result<(), Box<dyn std::error::Error>> {
    let fx = fixture()?;
    let scope = scope(&fx, "api.example.com")?;
    let partial = &fx.api.as_bytes()[..20];

    let (out, flagged) = run(&scope, &[partial]);

    assert_eq!(out, partial);
    assert!(flagged.is_empty());
    Ok(())
}

#[test]
fn the_real_value_is_not_rescanned() -> Result<(), Box<dyn std::error::Error>> {
    let mut registry = SentinelRegistry::new();
    let first = registry.register(
        SecretName::new("first")?,
        RealValue::new("short".to_string()),
        hosts("api.example.com")?,
    );
    let second = registry.register(
        SecretName::new("second")?,
        RealValue::new(format!("<{}>", first.as_str())),
        hosts("api.example.com")?,
    );
    let substitution = SecretSubstitution::new(Arc::new(registry));
    let scope = substitution
        .scope_for(&TlsTarget::new("api.example.com", 443))
        .ok_or("expected a value")?;

    let (out, _) = run(&scope, &[second.as_str().as_bytes()]);

    assert_eq!(out, format!("<{}>", first.as_str()).into_bytes());
    Ok(())
}

#[test]
fn covers_reports_only_hosts_a_binding_injects_at() -> Result<(), Box<dyn std::error::Error>> {
    let fx = fixture()?;

    assert!(fx.substitution.covers("api.example.com", 443));
    assert!(!fx.substitution.covers("example.net", 443));
    assert!(!fx.substitution.covers("not a host", 443));
    Ok(())
}

#[test]
fn debug_never_prints_a_value() -> Result<(), Box<dyn std::error::Error>> {
    let fx = fixture()?;
    let scope = scope(&fx, "api.example.com")?;

    let printed = format!("{:?} {:?}", fx.substitution, scope);

    assert!(!printed.contains(API_REAL));
    assert!(!printed.contains(&fx.api));
    Ok(())
}

#[test]
fn sentinels_of_differing_lengths_are_replaced_when_fed_one_byte_at_a_time() -> Result<(), Box<dyn std::error::Error>> {
    let mut registry = SentinelRegistry::new();
    let long_real = "L".repeat(90);
    let short = registry.register(
        SecretName::new("short")?,
        RealValue::new("s".repeat(20)),
        hosts("api.example.com")?,
    );
    let long = registry.register(
        SecretName::new("long")?,
        RealValue::new(long_real.clone()),
        hosts("api.example.com")?,
    );
    assert_ne!(short.as_str().len(), long.as_str().len());
    let substitution = SecretSubstitution::new(Arc::new(registry));
    let scope = substitution
        .scope_for(&TlsTarget::new("api.example.com", 443))
        .ok_or("expected a value")?;
    let body = format!(
        "a={} b={} c={} d={}",
        short.as_str(),
        long.as_str(),
        short.as_str(),
        long.as_str()
    );
    let chunks: Vec<&[u8]> = body.as_bytes().chunks(1).collect();

    let (out, flagged) = run(&scope, &chunks);

    let expected = format!("a={} b={long_real} c={} d={long_real}", "s".repeat(20), "s".repeat(20));
    assert_eq!(out, expected.into_bytes());
    assert!(flagged.is_empty());
    Ok(())
}

#[test]
fn sixty_four_kib_of_binary_without_a_sentinel_passes_through_identically() -> Result<(), Box<dyn std::error::Error>> {
    let fx = fixture()?;
    let scope = scope(&fx, "api.example.com")?;
    let mut state = 0x2545_f491_4f6c_dd1du64;
    let body: Vec<u8> = (0..64 * 1024)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 24) as u8
        })
        .collect();

    for chunk_size in [1024, 4093, 64 * 1024] {
        let chunks: Vec<&[u8]> = body.chunks(chunk_size).collect();
        let (out, flagged) = run(&scope, &chunks);
        assert_eq!(out, body, "chunk size {chunk_size}");
        assert!(flagged.is_empty());
    }
    Ok(())
}

#[test]
fn an_empty_body_stays_empty() -> Result<(), Box<dyn std::error::Error>> {
    let fx = fixture()?;
    let scope = scope(&fx, "api.example.com")?;

    let (out, flagged) = run(&scope, &[]);
    assert!(out.is_empty());
    assert!(flagged.is_empty());

    let (out, _) = run(&scope, &[b"", b""]);
    assert!(out.is_empty());
    Ok(())
}

#[test]
fn a_host_no_binding_injects_at_has_no_scope_to_substitute_with() -> Result<(), Box<dyn std::error::Error>> {
    let fx = fixture()?;

    let target = TlsTarget::new("unrelated.example.net", 443);

    assert!(
        !fx.substitution
            .scope_for(&target)
            .is_some_and(|scope| scope.injects_here())
    );
    Ok(())
}

#[test]
fn the_body_length_is_kept_only_when_every_sentinel_matches_its_real_length() -> TestResult<()> {
    let build = |reals: &[usize]| -> TestResult<SubstitutionScope> {
        let mut registry = SentinelRegistry::new();
        for (index, len) in reals.iter().enumerate() {
            registry.register(
                SecretName::new(format!("b{index}"))?,
                RealValue::new("r".repeat(*len)),
                hosts("api.example.com")?,
            );
        }
        SecretSubstitution::new(Arc::new(registry))
            .scope_for(&TlsTarget::new("api.example.com", 443))
            .ok_or_else(|| "expected a scope".into())
    };

    assert!(build(&[60, 70])?.keeps_body_length());
    assert!(!build(&[60, 20])?.keeps_body_length());
    assert!(!build(&[20])?.keeps_body_length());
    Ok(())
}

fn scoped(
    prefix: Option<&str>,
    methods: Option<Vec<InjectMethod>>,
    host: &str,
) -> TestResult<(SecretSubstitution, String)> {
    let mut registry = SentinelRegistry::new();
    let name = SecretName::new("scoped")?;
    let sentinel = registry.register(name.clone(), RealValue::new(API_REAL.to_string()), hosts(host)?);
    let path_prefix = match prefix {
        Some(prefix) => Some(InjectPathPrefix::new(prefix)?),
        None => None,
    };
    let methods = match methods {
        Some(methods) => Some(InjectMethodSet::new(methods)?),
        None => None,
    };
    registry.set_scope(
        &name,
        &InjectScope {
            path_prefix,
            methods,
            ..InjectScope::default()
        },
    );
    Ok((SecretSubstitution::new(Arc::new(registry)), sentinel.to_string()))
}

fn body_for(substitution: &SecretSubstitution, sentinel: &str, method: &str, path: &str) -> TestResult<(String, bool)> {
    let target = TlsTarget::new("api.example.com", 443);
    let scope = substitution
        .scope_for(&target)
        .ok_or("expected a scope")?
        .for_request(method, path);
    let (out, flagged) = run(&scope, &[sentinel.as_bytes()]);
    Ok((String::from_utf8(out)?, !flagged.is_empty()))
}

#[test]
fn a_path_scoped_binding_injects_under_v1_and_never_under_v10() -> Result<(), Box<dyn std::error::Error>> {
    let (substitution, sentinel) = scoped(Some("/v1"), None, "api.example.com")?;

    let (inside, _) = body_for(&substitution, &sentinel, "GET", "/v1/models")?;
    assert_eq!(inside, API_REAL);
    let (exact, _) = body_for(&substitution, &sentinel, "GET", "/v1")?;
    assert_eq!(exact, API_REAL);

    let (sibling, flagged) = body_for(&substitution, &sentinel, "GET", "/v10")?;
    assert_ne!(sibling, API_REAL);
    assert!(flagged);
    Ok(())
}

#[test]
fn a_method_scoped_binding_refuses_other_and_unknown_methods() -> Result<(), Box<dyn std::error::Error>> {
    let (substitution, sentinel) = scoped(None, Some(vec![InjectMethod::Get]), "api.example.com")?;

    let (get, _) = body_for(&substitution, &sentinel, "GET", "/x")?;
    assert_eq!(get, API_REAL);
    for method in ["POST", "get", "BREW"] {
        let (out, flagged) = body_for(&substitution, &sentinel, method, "/x")?;
        assert_ne!(out, API_REAL, "{method}");
        assert!(flagged, "{method}");
    }
    Ok(())
}

#[test]
fn a_scoped_binding_does_not_inject_before_a_request_is_known() -> Result<(), Box<dyn std::error::Error>> {
    let (substitution, _) = scoped(Some("/v1"), None, "api.example.com")?;
    let scope = substitution
        .scope_for(&TlsTarget::new("api.example.com", 443))
        .ok_or("expected a value")?;
    assert!(!scope.injects_here());
    Ok(())
}

#[test]
fn an_omitted_port_means_any_port_until_the_binding_opts_into_v2() -> Result<(), Box<dyn std::error::Error>> {
    let build = |version: InjectScopeVersion| -> TestResult<SecretSubstitution> {
        let mut registry = SentinelRegistry::new();
        let name = SecretName::new("p")?;
        registry.register(
            name.clone(),
            RealValue::new(API_REAL.to_string()),
            hosts("api.example.com")?,
        );
        registry.set_scope(
            &name,
            &InjectScope {
                scope_version: version,
                ..InjectScope::default()
            },
        );
        Ok(SecretSubstitution::new(Arc::new(registry)))
    };
    let v1 = build(InjectScopeVersion::V1)?;
    let v2 = build(InjectScopeVersion::V2)?;
    assert!(v1.covers("api.example.com", 8443));
    assert!(v1.covers("api.example.com", 443));
    assert!(!v2.covers("api.example.com", 8443));
    assert!(v2.covers("api.example.com", 443));
    Ok(())
}

#[derive(Default)]
struct Events {
    published: std::sync::Mutex<Vec<String>>,
}

#[async_trait]
impl crate::violation::ViolationSink for Events {
    async fn publish(&self, subject: String, _payload: Vec<u8>) -> Result<(), crate::violation::ViolationPublishError> {
        self.published
            .lock()
            .map_err(|_| crate::violation::ViolationPublishError("poisoned".to_string()))?
            .push(subject);
        Ok(())
    }
}

struct OnlyOffHost;

#[async_trait]
impl SentinelMisdirectionObserver for OnlyOffHost {
    async fn sentinel_off_host(&self, _host: &str, _port: u16, _binding: &SecretName) {}

    async fn compressed_response(&self, _host: &str, _port: u16, _binding: &SecretName) {}
}

fn refusal() -> TestResult<PairRefusal> {
    Ok(PairRefusal {
        refusal: crate::sigv4_resign::Sigv4RefusalError::MissingDate,
        binding: SecretName::new("api")?,
    })
}

#[tokio::test]
async fn the_violation_reporter_observes_every_kind_of_misdirection() -> TestResult<()> {
    let events = Arc::new(Events::default());
    let reporter = ViolationReporter::new(
        trogon_sandbox_contract::LeaseId::new("L1"),
        events.clone(),
        crate::violation::ViolationRateLimit::default(),
    );
    let observed = SecretSubstitution::new(fixture_registry()?).with_observer(Arc::new(reporter));
    let scope = observed
        .scope_for(&TlsTarget::new("api.example.com", 443))
        .ok_or("expected a scope")?;
    scope.report(&BTreeSet::from([SecretName::new("api")?])).await;
    scope.report_compressed_response().await;
    scope.report_sigv4_refused(&refusal()?).await;
    assert_eq!(events.published.lock().map_err(|_| "poisoned")?.len(), 3);
    Ok(())
}

#[tokio::test]
async fn an_observer_that_does_not_track_sigv4_ignores_a_refusal() -> TestResult<()> {
    let observed = SecretSubstitution::new(fixture_registry()?).with_observer(Arc::new(OnlyOffHost));
    let scope = observed
        .scope_for(&TlsTarget::new("api.example.com", 443))
        .ok_or("expected a scope")?;
    scope.report_sigv4_refused(&refusal()?).await;
    Ok(())
}

#[tokio::test]
async fn nothing_is_reported_without_an_observer() -> TestResult<()> {
    let fixture = fixture()?;
    let scope = scope(&fixture, "api.example.com")?;
    scope.report(&BTreeSet::from([SecretName::new("api")?])).await;
    scope.report_compressed_response().await;
    scope.report_sigv4_refused(&refusal()?).await;
    Ok(())
}

fn fixture_registry() -> TestResult<Arc<SentinelRegistry>> {
    let mut registry = SentinelRegistry::new();
    registry.register(
        SecretName::new("api")?,
        RealValue::new(API_REAL.to_string()),
        hosts("api.example.com")?,
    );
    Ok(Arc::new(registry))
}
