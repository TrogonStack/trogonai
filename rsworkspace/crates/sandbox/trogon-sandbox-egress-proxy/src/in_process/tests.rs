use super::*;
use crate::decider::Judgment;
use crate::request_decision::{DenyMark, HeaderEdits, RequestAllow, RequestDenial};
use crate::violation::ViolationSink;
use crate::violation_store::{CommandId, StoreViolationSink, ViolationStore};
use trogon_sandbox_contract::{EgressDecision, EgressViolation, EgressViolationReason};

fn lease() -> LeaseId {
    LeaseId::new("lease-1".to_string())
}

fn host(name: &str) -> Result<Host, Box<dyn std::error::Error>> {
    Ok(Host::new(name)?)
}

#[tokio::test]
async fn allow_and_deny_come_from_the_callback() -> Result<(), Box<dyn std::error::Error>> {
    let reasons = DenialReasons::new();
    let decider = in_process_decider(
        lease(),
        Arc::new(|question: AskQuestion| async move { question.host.to_string() == "ok.example.com" }),
        reasons,
        DeciderLimits::default(),
    );
    let port = Port::new(443)?;
    assert_eq!(
        decider.decide(&host("ok.example.com")?, port, None).await,
        Judgment::Allow
    );
    assert_eq!(
        decider.decide(&host("no.example.com")?, port, None).await,
        Judgment::Deny(EgressViolationReason::DeciderDenied)
    );
    Ok(())
}

#[tokio::test]
async fn a_denial_reason_is_remembered_for_the_violation() -> Result<(), Box<dyn std::error::Error>> {
    let reasons = DenialReasons::new();
    let decider = in_process_decider(
        lease(),
        Arc::new(|_: AskQuestion| async { AskVerdict::Deny(DenialReason::new("not on the list")) }),
        reasons.clone(),
        DeciderLimits::default(),
    );
    let port = Port::new(443)?;
    decider.decide(&host("x.example.com")?, port, None).await;
    let store = ViolationStore::new();
    let sink = StoreViolationSink::new(store.clone(), CommandId::new("c"), reasons);
    let violation = EgressViolation {
        lease_id: lease(),
        host: Some("x.example.com".to_string()),
        port: 443,
        dest_ip: None,
        decision: EgressDecision::Deny,
        reason: EgressViolationReason::DeciderDenied,
        binding: None,
        request: None,
        at: std::time::SystemTime::now(),
    };
    sink.publish(String::new(), serde_json::to_vec(&violation)?).await?;
    assert_eq!(
        store.violations(None)[0].line.as_str(),
        "deny network-outbound x.example.com:443 (not on the list)"
    );
    Ok(())
}

#[tokio::test]
async fn request_callback_filters_requests() -> Result<(), Box<dyn std::error::Error>> {
    let filter = CallbackRequestFilter::new(Arc::new(|request: RequestInfo| async move {
        request.path.starts_with("/ok")
    }));
    let host = host("api.example.com")?;
    let get = HttpMethod::new("GET")?;
    let view = |path| RequestView {
        host: &host,
        port: 80,
        method: &get,
        path,
        query: None,
        sni: None,
        headers: &[],
        body: None,
        scheme: RequestScheme::Https,
        forms: None,
    };
    assert_eq!(filter.decide(&view("/ok/1")).await, RequestDecision::Allow);
    assert_eq!(filter.decide(&view("/no")).await, RequestDecision::Deny);
    Ok(())
}

#[tokio::test]
async fn request_callback_sees_the_server_name_and_both_target_forms() -> Result<(), Box<dyn std::error::Error>> {
    let seen = Arc::new(std::sync::Mutex::new(None));
    let sink = seen.clone();
    let filter = CallbackRequestFilter::new(Arc::new(move |request: RequestInfo| {
        let sink = sink.clone();
        async move {
            if let Ok(mut slot) = sink.lock() {
                *slot = Some((request.sni, request.request_target));
            }
            true
        }
    }));
    let host = host("api.example.com")?;
    let get = HttpMethod::new("GET")?;
    let forms = TargetForms {
        spelled: "/a/../b".to_string(),
        normalized: "/b".to_string(),
        ambiguous: true,
    };
    let view = RequestView {
        host: &host,
        port: 443,
        method: &get,
        path: "/a/../b",
        query: None,
        sni: Some("api.example.com"),
        headers: &[],
        body: None,
        scheme: RequestScheme::Https,
        forms: Some(&forms),
    };
    filter.decide(&view).await;
    let (sni, request_target) = seen.lock().map_err(|_| "poisoned")?.take().ok_or("expected a value")?;
    assert_eq!(sni.as_deref(), Some("api.example.com"));
    assert_eq!(request_target, Some(forms));
    Ok(())
}

#[tokio::test]
async fn request_callback_returns_a_full_decision() -> Result<(), Box<dyn std::error::Error>> {
    let limited = RequestDecision::DenyWith(
        RequestDenial::new()
            .status(429)
            .reason("slow down")
            .mark(DenyMark::parse("rate_limited")?),
    );
    let edited = RequestDecision::AllowWith(
        RequestAllow::new().edits(HeaderEdits::new().set("X-Trace", "1")?.remove("Cookie")?),
    );
    let filter = CallbackRequestFilter::new(Arc::new(move |request: RequestInfo| {
        let decision = if request.path == "/limited" {
            limited.clone()
        } else {
            edited.clone()
        };
        async move { decision }
    }));
    let host = host("api.example.com")?;
    let get = HttpMethod::new("GET")?;
    let view = |path| RequestView {
        host: &host,
        port: 80,
        method: &get,
        path,
        query: None,
        sni: None,
        headers: &[],
        body: None,
        scheme: RequestScheme::Https,
        forms: None,
    };
    let RequestDecision::DenyWith(denial) = filter.decide(&view("/limited")).await else {
        return Err("expected a denial".into());
    };
    assert_eq!(denial.answer_status(), 429);
    assert_eq!(denial.denial_reason(), "slow down");
    let RequestDecision::AllowWith(allow) = filter.decide(&view("/x")).await else {
        return Err("expected an allow with edits".into());
    };
    let mut headers = vec![("Cookie".to_string(), "a=b".to_string())];
    allow.header_edits().apply(&mut headers);
    assert_eq!(headers, vec![("x-trace".to_string(), "1".to_string())]);
    Ok(())
}

fn sample_request() -> Result<RequestInfo, Box<dyn std::error::Error>> {
    Ok(RequestInfo {
        host: host("api.example.com")?,
        port: 443,
        method: HttpMethod::new("GET")?,
        path: "/".to_string(),
        headers: Vec::new(),
        body: RequestBodyReader::none(),
        scheme: RequestScheme::Https,
        sni: None,
        request_target: None,
    })
}

#[tokio::test]
async fn a_malformed_question_makes_the_transport_unavailable() -> Result<(), Box<dyn std::error::Error>> {
    let transport = InProcessDeciderTransport::new(Arc::new(|_: AskQuestion| async { true }), DenialReasons::new());
    let outcome = transport.request(String::new(), b"not json".to_vec()).await;
    assert!(matches!(outcome, Err(DeciderTransportError::Unavailable(_))));
    Ok(())
}

#[tokio::test]
async fn discarding_a_verdict_has_no_effect() -> Result<(), Box<dyn std::error::Error>> {
    DiscardVerdicts
        .amend(&host("api.example.com")?, Port::new(443)?, Decision::Allow)
        .await;
    Ok(())
}

struct LegacyAllow(bool);

#[async_trait]
impl RequestCallback for LegacyAllow {
    #[allow(deprecated)]
    async fn allow(&self, _request: RequestInfo) -> bool {
        self.0
    }
}

struct Undecided;

#[async_trait]
impl RequestCallback for Undecided {}

#[tokio::test]
async fn an_implementation_that_only_has_the_older_allow_is_still_asked() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(
        LegacyAllow(true).decide(sample_request()?).await,
        RequestDecision::Allow
    );
    assert_eq!(
        LegacyAllow(false).decide(sample_request()?).await,
        RequestDecision::Deny
    );
    Ok(())
}

#[tokio::test]
async fn an_implementation_with_neither_method_denies() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(Undecided.decide(sample_request()?).await, RequestDecision::Deny);
    Ok(())
}

#[tokio::test]
async fn a_closure_answers_the_older_allow_through_its_decision() -> Result<(), Box<dyn std::error::Error>> {
    let callback = |request: RequestInfo| async move { request.path == "/" };
    #[allow(deprecated)]
    let allowed = callback.allow(sample_request()?).await;
    assert!(allowed);
    Ok(())
}
