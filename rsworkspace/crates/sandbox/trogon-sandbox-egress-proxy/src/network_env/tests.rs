use super::*;
use trogon_sandbox_contract::DomainOnlyAllows;

fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
    move |name| {
        pairs
            .iter()
            .find(|(key, _)| *key == name)
            .map(|(_, value)| value.to_string())
    }
}

#[test]
fn nothing_set_keeps_the_defaults() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(options_from_env(env(&[]))?, NetworkOptions::default());
    Ok(())
}

#[test]
fn every_option_is_read() -> Result<(), Box<dyn std::error::Error>> {
    let options = options_from_env(env(&[
        (BINDING_TTL_SECS, "30"),
        (REVALIDATE_INTERVAL_SECS, "15"),
        (REQUIRE_TERMINATION_FOR_NAME_ONLY, "true"),
        (TLS_EXEMPT_HOSTS, "a.example.com, *.internal.test"),
        (TLS_EXEMPT_ADDRESSES, "10.0.0.0/8"),
    ]))?;
    assert_eq!(options.binding_ttl, Some(Duration::from_secs(30)));
    assert_eq!(options.revalidate_interval, Some(Duration::from_secs(15)));
    assert!(
        options.resweep_live_flows.is_enabled(),
        "an interval needs flow tracking"
    );
    assert_eq!(options.domain_only_allows, DomainOnlyAllows::RequireTermination);
    assert_eq!(options.tls_interception_exemptions.hostnames().len(), 2);
    assert_eq!(options.tls_interception_exemptions.addresses().len(), 1);
    Ok(())
}

#[test]
fn a_zero_interval_leaves_periodic_revalidation_off() -> Result<(), Box<dyn std::error::Error>> {
    let options = options_from_env(env(&[(REVALIDATE_INTERVAL_SECS, "0")]))?;
    assert_eq!(options.revalidate_interval, None);
    assert!(!options.resweep_live_flows.is_enabled());
    Ok(())
}

#[test]
fn resweep_alone_tracks_flows_without_a_timer() -> Result<(), Box<dyn std::error::Error>> {
    let options = options_from_env(env(&[(RESWEEP_LIVE_FLOWS, "1")]))?;
    assert!(options.resweep_live_flows.is_enabled());
    assert_eq!(options.revalidate_interval, None);
    Ok(())
}

#[test]
fn malformed_values_name_their_variable() -> Result<(), Box<dyn std::error::Error>> {
    for (name, value) in [
        (BINDING_TTL_SECS, "0"),
        (BINDING_TTL_SECS, "soon"),
        (REVALIDATE_INTERVAL_SECS, "-1"),
        (RESWEEP_LIVE_FLOWS, "yes"),
        (TLS_EXEMPT_ADDRESSES, "not-a-cidr"),
        (TLS_EXEMPT_HOSTS, "bad host"),
    ] {
        let Err(err) = options_from_env(env(&[(name, value)])) else {
            return Err(format!("{name}={value} should be rejected").into());
        };
        let reported = match err {
            NetworkEnvError::NotPositiveSeconds { name }
            | NetworkEnvError::NotSeconds { name }
            | NetworkEnvError::NotBoolean { name }
            | NetworkEnvError::InvalidHost { name, .. }
            | NetworkEnvError::InvalidRange { name, .. } => name,
        };
        assert_eq!(reported, name);
    }
    Ok(())
}

#[test]
fn only_the_set_variables_are_forwarded() -> Result<(), Box<dyn std::error::Error>> {
    let pairs = [(BINDING_TTL_SECS, "30"), ("UNRELATED", "x")];
    assert_eq!(forwarded(env(&pairs)), vec![(BINDING_TTL_SECS, "30".to_string())]);
    Ok(())
}
