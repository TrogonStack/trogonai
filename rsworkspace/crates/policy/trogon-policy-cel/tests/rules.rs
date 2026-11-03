#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

mod common;

use common::{cause_of, engine, gateway_facts};
use trogon_policy_cel::{CelVerdict, FactValue, RefusalCause, RequestFacts, RuntimeErrorKind};

/// What a rule over the gateway facts is expected to do.
enum Expect {
    Pass,
    False,
    NotBoolean(&'static str),
    Runtime(RuntimeErrorKind),
    Panicked,
}

use Expect::*;
use RuntimeErrorKind::*;

#[test]
fn a_rule_passes_only_when_it_is_the_boolean_true() {
    let table: Vec<(&str, &str, Expect)> = vec![
        ("true literal", "true", Pass),
        ("false literal", "false", False),
        ("equality on a field", r#"request.method == "POST""#, Pass),
        ("inequality on a field", r#"request.method != "POST""#, False),
        ("integer comparison", "request.port == 443 && caller.depth < 3", Pass),
        (
            "conjunction short of one",
            "request.port == 443 && caller.depth > 3",
            False,
        ),
        ("string prefix", r#"request.host.startsWith("api.")"#, Pass),
        ("string suffix", r#"request.host.endsWith(".example.com")"#, Pass),
        ("string contains", r#"request.host.contains("example")"#, Pass),
        ("regex match", r#"request.host.matches("^api\\.[a-z]+\\.com$")"#, Pass),
        ("regex mismatch", r#"request.host.matches("^internal")"#, False),
        (
            "membership in a literal list",
            r#"request.method in ["GET", "POST"]"#,
            Pass,
        ),
        ("membership in a fact list", r#""a" in request.tags"#, Pass),
        ("size of a list", "size(request.tags) == 2", Pass),
        ("every element", r#"request.tags.all(t, t != "x")"#, Pass),
        ("some element", r#"request.tags.exists(t, t == "b")"#, Pass),
        ("no element", r#"request.tags.exists(t, t == "z")"#, False),
        ("has on a present field", "has(request.host)", Pass),
        (
            "has on a missing field is false, not an error",
            "has(request.missing)",
            False,
        ),
        (
            "negated has guards a missing field",
            "!has(request.missing) || request.missing == 1",
            Pass,
        ),
        (
            "conditional",
            r#"request.port == 443 ? caller.agent == "a" : false"#,
            Pass,
        ),
        ("an integer is not a boolean", "request.port", NotBoolean("an integer")),
        ("a string is not a boolean", "request.method", NotBoolean("a string")),
        ("null is not a boolean", "null", NotBoolean("null")),
        ("a list is not a boolean", "request.tags", NotBoolean("a list")),
        ("a map is not a boolean", "caller", NotBoolean("a map")),
        ("a missing field", "request.nope == 1", Runtime(MissingField)),
        ("an unknown variable", "stranger == 1", Runtime(UndeclaredReference)),
        (
            "an unknown function",
            "frobnicate(request.port)",
            Runtime(UndeclaredReference),
        ),
        ("a type mismatch", r#"request.port + "x" == 1"#, Runtime(TypeMismatch)),
        ("division by zero", "request.port / 0 == 1", Runtime(Arithmetic)),
        ("remainder by zero", "request.port % 0 == 1", Runtime(Arithmetic)),
        (
            "integer overflow",
            "caller.depth * 9223372036854775807 > 0",
            Runtime(Arithmetic),
        ),
        (
            "a comparison across types",
            r#"request.port < "x""#,
            Runtime(TypeMismatch),
        ),
        ("a loop over a string", r#"request.method.all(c, c == "P")"#, Panicked),
        ("a loop over a number", "request.port.exists(c, c == 1)", Panicked),
    ];
    for (label, source, expect) in table {
        let verdict = engine(&[("r", source)]).evaluate(&gateway_facts());
        match expect {
            Pass => assert_eq!(verdict, CelVerdict::Pass, "{label}"),
            False => assert_eq!(cause_of(verdict), RefusalCause::False, "{label}"),
            NotBoolean(found) => assert_eq!(cause_of(verdict), RefusalCause::NotBoolean { found }, "{label}"),
            Runtime(kind) => assert_eq!(cause_of(verdict), RefusalCause::Runtime(kind), "{label}"),
            Panicked => assert_eq!(cause_of(verdict), RefusalCause::Panicked, "{label}"),
        }
    }
}

#[test]
fn a_rule_cannot_read_a_fact_the_host_did_not_supply() {
    let verdict = engine(&[("r", "request.port == 443")]).evaluate(&RequestFacts::new());
    assert_eq!(
        cause_of(verdict),
        RefusalCause::Runtime(RuntimeErrorKind::UndeclaredReference)
    );
}

#[test]
fn the_deepest_rule_that_compiles_evaluates_on_an_ordinary_stack() {
    let source = format!("{} > 0", vec!["1"; 60].join(" + "));
    assert_eq!(
        engine(&[("r", &source)]).evaluate(&RequestFacts::new()),
        CelVerdict::Pass
    );
}

#[test]
fn the_same_facts_give_the_same_verdict() {
    let engine = engine(&[("a", r#"request.method == "POST""#), ("b", "request.port == 80")]);
    let facts = gateway_facts();
    let first = engine.evaluate(&facts);
    for _ in 0..20 {
        assert_eq!(engine.evaluate(&facts), first);
    }
}

#[test]
fn a_rule_cannot_see_facts_left_over_from_another_request() {
    let engine = engine(&[("r", r#"has(request.host) && request.host == "api.example.com""#)]);
    assert_eq!(engine.evaluate(&gateway_facts()), CelVerdict::Pass);
    let other = RequestFacts::new().with(common::fact_name("request"), FactValue::map([("host", "elsewhere")]));
    assert_eq!(cause_of(engine.evaluate(&other)), RefusalCause::False);
}

#[test]
fn a_refusal_never_quotes_request_data() {
    let secret = "s3cr3t-token-value";
    let facts = RequestFacts::new().with(common::fact_name("request"), FactValue::map([("token", secret)]));
    let verdict = engine(&[("r", "request.token + 1 == 2")]).evaluate(&facts);
    let CelVerdict::Refused(refusal) = verdict else {
        panic!("expected a refusal");
    };
    assert!(!refusal.to_string().contains(secret));
    assert!(!format!("{refusal:?}").contains(secret));
}
