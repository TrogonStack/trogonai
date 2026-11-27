use super::*;

fn name(text: &str) -> Result<Host, Box<dyn std::error::Error>> {
    Ok(Host::new(text)?)
}

fn ip(text: &str) -> Result<IpAddr, Box<dyn std::error::Error>> {
    Ok(text.parse()?)
}

#[test]
fn a_binding_is_live_only_for_the_ttl() -> Result<(), Box<dyn std::error::Error>> {
    let cache = BindingCache::new(Duration::from_secs(30));
    let start = Instant::now();
    cache.record_at(&name("a.example.com")?, ip("203.0.113.7")?, start);
    assert!(cache.is_bound_at(
        &name("a.example.com")?,
        ip("203.0.113.7")?,
        start + Duration::from_secs(29)
    ));
    assert!(!cache.is_bound_at(
        &name("a.example.com")?,
        ip("203.0.113.7")?,
        start + Duration::from_secs(30)
    ));
    Ok(())
}

#[test]
fn two_names_on_one_shared_address_are_bound_separately() -> Result<(), Box<dyn std::error::Error>> {
    let cache = BindingCache::default();
    cache.record(&name("allowed.example.com")?, ip("203.0.113.7")?);
    assert!(cache.is_bound(&name("allowed.example.com")?, ip("203.0.113.7")?));
    assert!(!cache.is_bound(&name("other.example.net")?, ip("203.0.113.7")?));
    Ok(())
}

#[test]
fn a_mapped_address_is_the_ipv4_address_it_spells() -> Result<(), Box<dyn std::error::Error>> {
    let cache = BindingCache::default();
    cache.record(&name("a.example.com")?, ip("::ffff:203.0.113.7")?);
    assert!(cache.is_bound(&name("a.example.com")?, ip("203.0.113.7")?));
    Ok(())
}

#[test]
fn an_ip_literal_is_never_a_dns_observation() -> Result<(), Box<dyn std::error::Error>> {
    let cache = BindingCache::default();
    cache.record(&name("203.0.113.7")?, ip("203.0.113.7")?);
    assert!(!cache.is_bound(&name("203.0.113.7")?, ip("203.0.113.7")?));
    Ok(())
}

#[test]
fn the_cache_is_bounded_and_drops_the_oldest_first() -> Result<(), Box<dyn std::error::Error>> {
    let cache = BindingCache::new(Duration::from_secs(3600));
    let start = Instant::now();
    let addr = |i: usize| IpAddr::from(std::net::Ipv6Addr::from(i as u128 + 1));
    for i in 0..=MAX_BINDINGS {
        cache.record_at(
            &name("a.example.com")?,
            addr(i),
            start + Duration::from_millis(i as u64),
        );
    }
    let now = start + Duration::from_secs(1000);
    assert!(!cache.is_bound_at(&name("a.example.com")?, addr(0), now));
    assert!(cache.is_bound_at(&name("a.example.com")?, addr(MAX_BINDINGS), now));
    Ok(())
}

#[test]
fn the_cache_reports_its_ttl() {
    assert_eq!(BindingCache::new(Duration::from_secs(7)).ttl(), Duration::from_secs(7));
}
