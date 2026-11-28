use trogon_sandbox_contract::PathPrefix;

use super::*;
use crate::request_decision::HeaderEdits;

struct Fixed(RequestDecision);

#[async_trait]
impl RequestFilter for Fixed {
    async fn decide(&self, _request: &RequestView<'_>) -> RequestDecision {
        self.0.clone()
    }
}

fn view<'a>(host: &'a Host, method: &'a HttpMethod, path: &'a str) -> RequestView<'a> {
    RequestView {
        host,
        port: 80,
        method,
        path,
        query: None,
        sni: None,
        headers: &[],
        body: None,
        scheme: RequestScheme::Https,
        forms: None,
    }
}

fn policy(request_rules: Vec<RequestRule>) -> NetworkPolicy {
    NetworkPolicy::UserDefined {
        rules: Vec::new(),
        allow_ranges: Vec::new(),
        deny_ranges: Vec::new(),
        denied_domains: Vec::new(),
        opaque_tunnels: trogon_sandbox_contract::OpaqueTunnelPolicy::Allow,
        request_rules,
    }
}

#[tokio::test]
async fn a_policy_without_request_rules_has_no_filter() -> Result<(), Box<dyn std::error::Error>> {
    assert!(PolicyRequestFilter::from_policy(&policy(Vec::new())).is_none());
    assert!(PolicyRequestFilter::from_policy(&NetworkPolicy::AllowAll).is_none());
    Ok(())
}

#[tokio::test]
async fn the_policy_filter_applies_its_rules() -> Result<(), Box<dyn std::error::Error>> {
    let filter = PolicyRequestFilter::from_policy(&policy(vec![RequestRule {
        action: RequestRuleAction::Deny,
        domains: Vec::new(),
        methods: vec![HttpMethod::new("DELETE")?],
        path_prefix: Some(PathPrefix::new("/admin")?),
        headers: Vec::new(),
    }]))
    .ok_or("expected a value")?;
    let host = Host::new("api.example.com")?;
    let delete = HttpMethod::new("DELETE")?;
    let get = HttpMethod::new("GET")?;
    assert_eq!(
        filter.decide(&view(&host, &delete, "/admin/x")).await,
        RequestDecision::Deny
    );
    assert_eq!(
        filter.decide(&view(&host, &get, "/admin/x")).await,
        RequestDecision::Allow
    );
    assert_eq!(
        filter.decide(&view(&host, &delete, "/public")).await,
        RequestDecision::Allow
    );
    Ok(())
}

#[tokio::test]
async fn every_filter_must_allow() -> Result<(), Box<dyn std::error::Error>> {
    let host = Host::new("a.test")?;
    let get = HttpMethod::new("GET")?;
    let both_allow = AllOf(vec![
        Arc::new(Fixed(RequestDecision::Allow)),
        Arc::new(Fixed(RequestDecision::Allow)),
    ]);
    let one_denies = AllOf(vec![
        Arc::new(Fixed(RequestDecision::Allow)),
        Arc::new(Fixed(RequestDecision::Deny)),
    ]);
    assert_eq!(both_allow.decide(&view(&host, &get, "/")).await, RequestDecision::Allow);
    assert_eq!(one_denies.decide(&view(&host, &get, "/")).await, RequestDecision::Deny);
    Ok(())
}

#[test]
fn schemes_and_verdicts_describe_themselves() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(RequestScheme::Http.as_str(), "http");
    assert_eq!(RequestScheme::Https.as_str(), "https");
    assert!(RequestDecision::from(true).is_allow());
    assert!(!RequestDecision::from(false).is_allow());
    assert!(RequestDecision::AllowWith(RequestAllow::new()).is_allow());
    Ok(())
}

#[tokio::test]
async fn all_of_asks_every_filter_in_turn() -> Result<(), Box<dyn std::error::Error>> {
    let host = Host::new("a.test")?;
    let get = HttpMethod::new("GET")?;
    let both_allow = all_of(vec![
        Arc::new(Fixed(RequestDecision::Allow)),
        Arc::new(Fixed(RequestDecision::Allow)),
    ]);
    assert_eq!(both_allow.decide(&view(&host, &get, "/")).await, RequestDecision::Allow);
    let edits = RequestAllow::new().edits(HeaderEdits::new().remove("a")?);
    let editing = all_of(vec![Arc::new(Fixed(RequestDecision::AllowWith(edits.clone())))]);
    assert_eq!(
        editing.decide(&view(&host, &get, "/")).await,
        RequestDecision::AllowWith(edits)
    );
    Ok(())
}

#[tokio::test]
async fn assembled_checks_combine_the_policy_rules_and_the_custom_filter() -> Result<(), Box<dyn std::error::Error>> {
    let rules = policy(vec![RequestRule {
        action: RequestRuleAction::Deny,
        domains: Vec::new(),
        methods: vec![HttpMethod::new("DELETE")?],
        path_prefix: None,
        headers: Vec::new(),
    }]);
    let custom: Arc<dyn RequestFilter> = Arc::new(Fixed(RequestDecision::Allow));
    assert!(RequestChecks::assembled(&policy(Vec::new()), None, None, false, None, None).is_none());
    let alone = RequestChecks::assembled(&policy(Vec::new()), Some(&custom), None, false, None, None);
    assert!(alone.is_some());
    let both = RequestChecks::assembled(&rules, Some(&custom), None, true, None, None).ok_or("expected checks")?;
    assert!(both.plaintext_header_set());
    assert_eq!(format!("{both:?}"), "RequestChecks { .. }");
    Ok(())
}

#[test]
fn checks_carry_their_mutators_and_the_plaintext_switch() -> Result<(), Box<dyn std::error::Error>> {
    let mutator: Arc<dyn HeaderMutator> = Arc::new(|_: &mut Vec<(String, String)>, _: &Host| {});
    let checks = RequestChecks::new(Arc::new(Fixed(RequestDecision::Allow)));
    assert!(!checks.plaintext_header_set());
    assert!(checks.mutator().is_none());
    assert!(checks.plaintext_mutator().is_none());
    let checks = checks
        .with_plaintext_header_set(true)
        .with_header_mutator(Arc::clone(&mutator))
        .with_plaintext_header_mutator(mutator);
    assert!(checks.plaintext_header_set());
    assert!(checks.mutator().is_some());
    assert!(checks.plaintext_mutator().is_some());
    Ok(())
}
