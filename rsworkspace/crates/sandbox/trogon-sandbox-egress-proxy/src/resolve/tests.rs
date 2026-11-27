use super::constants::FLOOR_RANGES;
use super::*;
use trogon_sandbox_contract::{AllowRange, DomainRule, DomainRuleAction, IpRange};

use AddressDenialClass::*;

struct FixedInterfaces(Vec<IpAddr>);

impl LocalInterfaces for FixedInterfaces {
    fn addresses(&self) -> io::Result<Vec<IpAddr>> {
        Ok(self.0.clone())
    }
}

struct FailingInterfaces;

impl LocalInterfaces for FailingInterfaces {
    fn addresses(&self) -> io::Result<Vec<IpAddr>> {
        Err(io::ErrorKind::PermissionDenied.into())
    }
}

struct FixedLookup(Vec<IpAddr>);

#[async_trait]
impl NameLookup for FixedLookup {
    async fn lookup(&self, _name: &str, _port: Port) -> io::Result<Vec<IpAddr>> {
        Ok(self.0.clone())
    }
}

fn ip(text: &str) -> Result<IpAddr, Box<dyn std::error::Error>> {
    Ok(parse_address(text)?)
}

fn ips(texts: &[&str]) -> Result<Vec<IpAddr>, Box<dyn std::error::Error>> {
    texts.iter().map(|text| ip(text)).collect()
}

fn floor(local: &[&str]) -> Result<SsrfFloor, Box<dyn std::error::Error>> {
    Ok(SsrfFloor::new(Arc::new(FixedInterfaces(ips(local)?))))
}

fn allow(ranges: &[&str]) -> Result<AllowRanges, Box<dyn std::error::Error>> {
    Ok(AllowRanges::from_policy(&NetworkPolicy::UserDefined {
        rules: Vec::<DomainRule>::new(),
        allow_ranges: ranges.iter().map(|r| AllowRange::new(*r)).collect::<Result<_, _>>()?,
        deny_ranges: vec![],
        denied_domains: vec![],
        opaque_tunnels: Default::default(),
        request_rules: vec![],
    }))
}

fn denial(address: &str) -> Result<Option<AddressDenialClass>, Box<dyn std::error::Error>> {
    Ok(floor(&[])?.vet(ip(address)?, &AllowRanges::default()).err())
}

#[test]
fn denies_the_built_in_set() -> Result<(), Box<dyn std::error::Error>> {
    let cases = [
        ("127.0.0.1", Loopback),
        ("127.255.255.254", Loopback),
        ("::1", Loopback),
        ("0.0.0.0", Unspecified),
        ("0.1.2.3", Unspecified),
        ("::", Unspecified),
        ("169.254.169.254", LinkLocal),
        ("fd00:ec2::254", CloudMetadata),
        ("fd00:ec2::23", CloudMetadata),
        ("100.100.100.200", CloudMetadata),
        ("168.63.129.16", CloudMetadata),
        ("192.0.0.192", CloudMetadata),
        ("fd20:ce::254", CloudMetadata),
        ("fd00:c1::a9fe:a9fe", CloudMetadata),
        ("fd00:42::42", CloudMetadata),
        ("fd00:a9fe:a9fe::1", CloudMetadata),
        ("fd00:100::100:200", CloudMetadata),
        ("169.254.1.1", LinkLocal),
        ("fe80::1", LinkLocal),
        ("224.0.0.1", Multicast),
        ("239.255.255.250", Multicast),
        ("ff02::1", Multicast),
        ("255.255.255.255", Broadcast),
        ("10.1.2.3", PrivateUse),
        ("172.16.0.1", PrivateUse),
        ("172.31.255.255", PrivateUse),
        ("192.168.1.1", PrivateUse),
        ("100.64.0.1", PrivateUse),
        ("100.127.255.254", PrivateUse),
        ("fc00::1", PrivateUse),
        ("fdab::1", PrivateUse),
    ];
    for (address, expected) in cases {
        assert_eq!(denial(address)?, Some(expected), "{address}");
    }
    Ok(())
}

#[test]
fn permits_public_addresses_at_the_edges_of_denied_ranges() -> Result<(), Box<dyn std::error::Error>> {
    for address in [
        "8.8.8.8",
        "1.1.1.1",
        "172.15.255.255",
        "172.32.0.1",
        "100.63.255.255",
        "100.128.0.1",
        "169.253.255.255",
        "126.255.255.255",
        "2606:4700:4700::1111",
        "fe00::1",
    ] {
        assert_eq!(denial(address)?, None, "{address}");
    }
    Ok(())
}

#[test]
fn judges_ipv4_mapped_addresses_as_the_ipv4_address() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(denial("::ffff:127.0.0.1")?, Some(Loopback));
    assert_eq!(denial("::ffff:7f00:1")?, Some(Loopback));
    assert_eq!(denial("::ffff:169.254.169.254")?, Some(LinkLocal));
    assert_eq!(denial("::ffff:10.0.0.1")?, Some(PrivateUse));
    assert_eq!(denial("::ffff:8.8.8.8")?, None);
    Ok(())
}

#[test]
fn judges_addresses_that_carry_an_ipv4_address_as_that_address_too() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(denial("::127.0.0.1")?, Some(Loopback));
    assert_eq!(denial("::a9fe:a9fe")?, Some(LinkLocal));
    assert_eq!(denial("::ffff:0:7f00:1")?, Some(Loopback));
    assert_eq!(denial("64:ff9b::7f00:1")?, Some(Loopback));
    assert_eq!(denial("64:ff9b::a9fe:a9fe")?, Some(LinkLocal));
    assert_eq!(denial("64:ff9b::a00:1")?, Some(PrivateUse));
    assert_eq!(denial("2002:7f00:1::")?, Some(Loopback));
    assert_eq!(denial("2002:a9fe:a9fe::1")?, Some(LinkLocal));
    assert_eq!(denial("64:ff9b::808:808")?, None);
    assert_eq!(denial("2002:808:808::1")?, None);
    Ok(())
}

#[test]
fn a_carried_ipv4_address_never_earns_a_carve_out() -> Result<(), Box<dyn std::error::Error>> {
    let exceptions = allow(&["0.0.0.1/32"])?;
    assert_eq!(
        floor(&[])?.vet(ip("::1")?, &exceptions),
        Err(Loopback),
        "::1 carries 0.0.0.1 but must not ride its carve-out"
    );
    let exceptions = allow(&["127.0.0.1/32"])?;
    assert_eq!(floor(&[])?.vet(ip("64:ff9b::7f00:1")?, &exceptions), Err(Loopback));
    Ok(())
}

#[test]
fn zone_ids_are_dropped_before_judging() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(ip("fe80::1%en0")?, ip("fe80::1")?);
    assert_eq!(denial("fe80::1%en0")?, Some(LinkLocal));
    assert_eq!(parse_address("::1%lo0"), Ok(ip("::1")?));
    assert!(parse_address("not-an-ip").is_err());
    assert!(parse_address("%en0").is_err());
    Ok(())
}

#[test]
fn denies_this_hosts_own_interface_addresses_in_either_spelling() -> Result<(), Box<dyn std::error::Error>> {
    let floor = floor(&["203.0.113.9", "2001:db8::9"])?;
    let none = AllowRanges::default();
    assert_eq!(floor.vet(ip("203.0.113.9")?, &none), Err(OwnInterface));
    assert_eq!(floor.vet(ip("::ffff:203.0.113.9")?, &none), Err(OwnInterface));
    assert_eq!(floor.vet(ip("2001:db8::9")?, &none), Err(OwnInterface));
    assert_eq!(floor.vet(ip("64:ff9b::cb00:7109")?, &none), Err(OwnInterface));
    assert!(floor.vet(ip("203.0.113.10")?, &none).is_ok());
    Ok(())
}

#[test]
fn the_interface_addresses_are_read_again_at_each_judgement() -> Result<(), Box<dyn std::error::Error>> {
    struct Changing(std::sync::Mutex<Vec<IpAddr>>);

    impl LocalInterfaces for Changing {
        fn addresses(&self) -> io::Result<Vec<IpAddr>> {
            Ok(self.0.lock().map_err(|_| io::Error::other("poisoned"))?.clone())
        }
    }

    let interfaces = Arc::new(Changing(std::sync::Mutex::new(vec![ip("203.0.113.9")?])));
    let floor = SsrfFloor::new(interfaces.clone());
    let none = AllowRanges::default();
    assert_eq!(floor.vet(ip("203.0.113.9")?, &none), Err(OwnInterface));
    *interfaces.0.lock().map_err(|_| "poisoned")? = vec![];
    assert!(floor.vet(ip("203.0.113.9")?, &none).is_ok());
    *interfaces.0.lock().map_err(|_| "poisoned")? = vec![ip("203.0.113.9")?];
    assert_eq!(floor.vet(ip("203.0.113.9")?, &none), Err(OwnInterface));
    Ok(())
}

#[test]
fn mapped_interface_addresses_are_judged_as_ipv4() -> Result<(), Box<dyn std::error::Error>> {
    let floor = floor(&["::ffff:203.0.113.9"])?;
    assert_eq!(
        floor.vet(ip("203.0.113.9")?, &AllowRanges::default()),
        Err(OwnInterface)
    );
    Ok(())
}

#[test]
fn failing_to_enumerate_interfaces_refuses_the_address() -> Result<(), Box<dyn std::error::Error>> {
    let floor = SsrfFloor::new(Arc::new(FailingInterfaces));
    assert_eq!(floor.vet(ip("8.8.8.8")?, &AllowRanges::default()), Err(OwnInterface));
    Ok(())
}

#[test]
fn explicit_allow_ranges_carve_out_exceptions() -> Result<(), Box<dyn std::error::Error>> {
    let floor = floor(&["203.0.113.9"])?;
    let exceptions = allow(&["10.0.0.0/8", "127.0.0.1/32", "203.0.113.9/32", "fc00::/7"])?;
    for address in ["10.9.9.9", "127.0.0.1", "203.0.113.9", "fc00::5", "::ffff:10.1.1.1"] {
        assert!(floor.vet(ip(address)?, &exceptions).is_ok(), "{address}");
    }
    for (address, expected) in [
        ("127.0.0.2", Loopback),
        ("169.254.169.254", LinkLocal),
        ("192.168.0.1", PrivateUse),
    ] {
        assert_eq!(floor.vet(ip(address)?, &exceptions), Err(expected), "{address}");
    }
    Ok(())
}

#[test]
fn malformed_ranges_cannot_be_built_and_absent_ranges_grant_nothing() -> Result<(), Box<dyn std::error::Error>> {
    for malformed in ["not a range", "10.0.0.0/33", "", "10.0.0.5", "10.0.0.5:8080"] {
        assert!(IpRange::new(malformed).is_err(), "{malformed}");
    }
    let exceptions = allow(&[])?;
    assert_eq!(exceptions, AllowRanges::default());
    assert_eq!(floor(&[])?.vet(ip("10.0.0.1")?, &exceptions), Err(PrivateUse));
    assert_eq!(
        AllowRanges::from_policy(&NetworkPolicy::AllowAll),
        AllowRanges::default()
    );
    assert_eq!(allow(&["10.0.0.5"])?, allow(&["10.0.0.5/32"])?);
    Ok(())
}

#[test]
fn a_range_with_a_port_carves_out_that_port_only() -> Result<(), Box<dyn std::error::Error>> {
    let floor = floor(&[])?;
    let exceptions = allow(&["10.0.0.5:8080", "[fc00::1]:9", "10.0.0.6"])?;
    assert!(floor.vet(ip("10.0.0.5")?, &exceptions.for_port(8080)).is_ok());
    assert_eq!(floor.vet(ip("10.0.0.5")?, &exceptions.for_port(22)), Err(PrivateUse));
    assert!(floor.vet(ip("fc00::1")?, &exceptions.for_port(9)).is_ok());
    assert_eq!(floor.vet(ip("fc00::1")?, &exceptions.for_port(8080)), Err(PrivateUse));
    assert!(floor.vet(ip("10.0.0.6")?, &exceptions.for_port(22)).is_ok());
    Ok(())
}

#[test]
fn a_mapped_range_entry_does_not_widen_to_other_addresses() -> Result<(), Box<dyn std::error::Error>> {
    let exceptions = allow(&["::ffff:10.0.0.1/128"])?;
    assert_eq!(floor(&[])?.vet(ip("10.0.0.2")?, &exceptions), Err(PrivateUse));
    assert!(floor(&[])?.vet(ip("10.0.0.1")?, &exceptions).is_ok());
    assert_eq!(allow(&["::ffff:0:0/80"])?, AllowRanges::default());
    Ok(())
}

fn resolver(answer: &[&str]) -> Result<VettingResolver, Box<dyn std::error::Error>> {
    Ok(VettingResolver::new(Arc::new(FixedLookup(ips(answer)?)), floor(&[])?))
}

fn name() -> Result<Host, Box<dyn std::error::Error>> {
    Ok(Host::new("allowed.test")?)
}

fn port() -> Result<Port, Box<dyn std::error::Error>> {
    Ok(Port::new(443)?)
}

#[tokio::test]
async fn a_name_resolving_only_to_denied_addresses_is_refused_with_the_top_class()
-> Result<(), Box<dyn std::error::Error>> {
    let outcome = resolver(&["10.0.0.1", "169.254.169.254", "192.168.0.1"])?
        .resolve(&name()?, port()?, &AllowRanges::default())
        .await;
    assert!(matches!(
        outcome,
        Err(ResolveError::Denied {
            class: LinkLocal,
            addr,
            ..
        }) if addr == ip("169.254.169.254")?
    ));
    Ok(())
}

fn listed(ranges: &[&str]) -> Result<AllowRanges, Box<dyn std::error::Error>> {
    Ok(AllowRanges::from_ranges(
        &ranges.iter().map(|r| IpRange::new(*r)).collect::<Result<Vec<_>, _>>()?,
    ))
}

#[tokio::test]
async fn a_listed_address_denies_a_name_that_resolves_to_it() -> Result<(), Box<dyn std::error::Error>> {
    let outcome = resolver(&["93.184.216.34"])?
        .with_listed_addresses(listed(&["93.184.216.0/24"])?)
        .resolve(&name()?, port()?, &AllowRanges::default())
        .await;
    assert!(matches!(outcome, Err(ResolveError::Denied { class: Listed, .. })));
    Ok(())
}

#[tokio::test]
async fn a_listed_address_never_judges_an_ip_literal() -> Result<(), Box<dyn std::error::Error>> {
    let literal = Host::new("93.184.216.34")?;
    let outcome = resolver(&["93.184.216.34"])?
        .with_listed_addresses(listed(&["93.184.216.0/24"])?)
        .resolve(&literal, port()?, &AllowRanges::default())
        .await;
    assert!(outcome.is_ok());
    Ok(())
}

#[tokio::test]
async fn the_allow_carve_out_wins_over_a_listed_address() -> Result<(), Box<dyn std::error::Error>> {
    let outcome = resolver(&["93.184.216.34"])?
        .with_listed_addresses(listed(&["93.184.216.0/24"])?)
        .resolve(&name()?, port()?, &allow(&["93.184.216.34/32"])?)
        .await;
    assert!(outcome.is_ok());
    Ok(())
}

#[tokio::test]
async fn the_first_surviving_address_is_the_one_returned() -> Result<(), Box<dyn std::error::Error>> {
    let vetted = resolver(&["127.0.0.1", "93.184.216.34", "93.184.216.35"])?
        .resolve(&name()?, port()?, &AllowRanges::default())
        .await?;
    assert_eq!(vetted.ip(), ip("93.184.216.34")?);
    assert_eq!(vetted.with_port(443).socket_addr(), "93.184.216.34:443".parse()?);
    Ok(())
}

#[tokio::test]
async fn an_empty_answer_is_unresolved_not_denied() -> Result<(), Box<dyn std::error::Error>> {
    let outcome = resolver(&[])?.resolve(&name()?, port()?, &AllowRanges::default()).await;
    assert!(matches!(outcome, Err(ResolveError::Unresolved(_))));
    Ok(())
}

#[tokio::test]
async fn ip_literals_are_vetted_too() -> Result<(), Box<dyn std::error::Error>> {
    let resolver = resolver(&[])?;
    let none = AllowRanges::default();
    let metadata = Host::new("169.254.169.254")?;
    assert!(matches!(
        resolver.resolve(&metadata, port()?, &none).await,
        Err(ResolveError::Denied { class: LinkLocal, .. })
    ));
    let carved = allow(&["169.254.169.254/32"])?;
    assert!(resolver.resolve(&metadata, port()?, &carved).await.is_ok());
    Ok(())
}

#[tokio::test]
async fn a_carve_out_lets_a_name_reach_loopback() -> Result<(), Box<dyn std::error::Error>> {
    let exceptions = allow(&["127.0.0.0/8"])?;
    let vetted = resolver(&["127.0.0.1"])?
        .resolve(&name()?, port()?, &exceptions)
        .await?;
    assert_eq!(vetted.ip(), ip("127.0.0.1")?);
    Ok(())
}

#[test]
fn the_system_interfaces_include_loopback() -> Result<(), Box<dyn std::error::Error>> {
    let addresses = SystemInterfaces.addresses()?;
    assert!(addresses.contains(&IpAddr::V4(Ipv4Addr::LOCALHOST)));
    Ok(())
}

#[test]
fn an_explicit_catch_all_range_lifts_the_floor() -> Result<(), Box<dyn std::error::Error>> {
    let policy = NetworkPolicy::UserDefined {
        rules: vec![DomainRule {
            domain: "allowed.test".parse()?,
            action: DomainRuleAction::PassThrough,
        }],
        allow_ranges: vec![AllowRange::new("0.0.0.0/0")?],
        deny_ranges: vec![],
        denied_domains: vec![],
        opaque_tunnels: Default::default(),
        request_rules: vec![],
    };
    let exceptions = AllowRanges::from_policy(&policy);
    assert!(floor(&[])?.vet(ip("127.0.0.1")?, &exceptions).is_ok());
    Ok(())
}

struct FailingLookup;

#[async_trait]
impl NameLookup for FailingLookup {
    async fn lookup(&self, _name: &str, _port: Port) -> io::Result<Vec<IpAddr>> {
        Err(io::ErrorKind::NotFound.into())
    }
}

#[tokio::test]
async fn a_destination_among_the_answers_is_bound_to_the_name() -> Result<(), Box<dyn std::error::Error>> {
    let result = resolver(&["203.0.113.7", "203.0.113.8"])?
        .confirm_binding(&name()?, port()?, ip("203.0.113.8")?)
        .await;
    assert!(result.is_ok());
    Ok(())
}

#[tokio::test]
async fn a_destination_outside_the_answers_is_not_bound_to_the_name() -> Result<(), Box<dyn std::error::Error>> {
    let result = resolver(&["203.0.113.7"])?
        .confirm_binding(&name()?, port()?, ip("198.51.100.9")?)
        .await;
    assert!(matches!(result, Err(NameBindingError::NotAnAnswer)));
    Ok(())
}

#[tokio::test]
async fn a_mapped_destination_matches_its_ipv4_answer_and_the_reverse() -> Result<(), Box<dyn std::error::Error>> {
    assert!(
        resolver(&["203.0.113.7"])?
            .confirm_binding(&name()?, port()?, ip("::ffff:203.0.113.7")?)
            .await
            .is_ok()
    );
    assert!(
        resolver(&["::ffff:203.0.113.7"])?
            .confirm_binding(&name()?, port()?, ip("203.0.113.7")?)
            .await
            .is_ok()
    );
    Ok(())
}

#[tokio::test]
async fn an_empty_answer_set_binds_nothing() -> Result<(), Box<dyn std::error::Error>> {
    let result = resolver(&[])?
        .confirm_binding(&name()?, port()?, ip("203.0.113.7")?)
        .await;
    assert!(matches!(result, Err(NameBindingError::NotAnAnswer)));
    Ok(())
}

#[tokio::test]
async fn a_failed_lookup_binds_nothing() -> Result<(), Box<dyn std::error::Error>> {
    let resolver = VettingResolver::new(Arc::new(FailingLookup), floor(&[])?);
    let result = resolver.confirm_binding(&name()?, port()?, ip("203.0.113.7")?).await;
    assert!(matches!(result, Err(NameBindingError::Unresolved(_))));
    Ok(())
}

#[tokio::test]
async fn an_ip_literal_name_binds_only_to_itself() -> Result<(), Box<dyn std::error::Error>> {
    let literal = Host::new("203.0.113.7")?;
    let resolver = VettingResolver::new(Arc::new(FailingLookup), floor(&[])?);
    assert!(
        resolver
            .confirm_binding(&literal, port()?, ip("203.0.113.7")?)
            .await
            .is_ok()
    );
    assert!(matches!(
        resolver.confirm_binding(&literal, port()?, ip("203.0.113.8")?).await,
        Err(NameBindingError::NotAnAnswer)
    ));
    Ok(())
}

async fn resolve_name(
    name: &str,
    answers: &[&str],
    loopback_names: bool,
    exceptions: &AllowRanges,
) -> Result<Result<VettedAddr, ResolveError>, Box<dyn std::error::Error>> {
    let mut resolver = VettingResolver::new(Arc::new(FixedLookup(ips(answers)?)), floor(&["192.0.2.10"])?);
    if loopback_names {
        resolver = resolver.with_loopback_names();
    }
    Ok(resolver.resolve(&Host::new(name)?, Port::new(443)?, exceptions).await)
}

async fn resolve_with_private_names(
    host: &str,
    answers: &[&str],
) -> Result<Result<VettedAddr, ResolveError>, Box<dyn std::error::Error>> {
    Ok(
        VettingResolver::new(Arc::new(FixedLookup(ips(answers)?)), floor(&["192.0.2.10"])?)
            .with_private_ranges_by_name()
            .resolve(&Host::new(host)?, Port::new(443)?, &AllowRanges::default())
            .await,
    )
}

#[tokio::test]
async fn private_ranges_are_refused_for_names_by_default() -> Result<(), Box<dyn std::error::Error>> {
    for answer in ["10.1.2.3", "172.16.0.1", "192.168.1.1", "100.64.0.1", "fd00::1"] {
        assert!(matches!(
            resolve_name("intranet.test", &[answer], false, &AllowRanges::default()).await?,
            Err(ResolveError::Denied {
                class: AddressDenialClass::PrivateUse,
                ..
            })
        ));
    }
    Ok(())
}

#[tokio::test]
async fn a_name_may_resolve_into_private_ranges_when_asked() -> Result<(), Box<dyn std::error::Error>> {
    for answer in ["10.1.2.3", "172.16.0.1", "192.168.1.1", "100.64.0.1", "fd00::1"] {
        assert_eq!(
            resolve_with_private_names("intranet.test", &[answer]).await??,
            VettedAddr::vetted(ip(answer)?)
        );
    }
    Ok(())
}

#[tokio::test]
async fn private_names_do_not_loosen_any_other_class() -> Result<(), Box<dyn std::error::Error>> {
    for (answer, class) in [
        ("127.0.0.1", AddressDenialClass::Loopback),
        ("169.254.169.254", AddressDenialClass::LinkLocal),
        ("169.254.1.1", AddressDenialClass::LinkLocal),
        ("224.0.0.1", AddressDenialClass::Multicast),
        ("0.0.0.0", AddressDenialClass::Unspecified),
        ("192.0.2.10", AddressDenialClass::OwnInterface),
    ] {
        match resolve_with_private_names("intranet.test", &[answer]).await? {
            Err(ResolveError::Denied { class: got, .. }) => assert_eq!(got, class),
            other => return Err(format!("{answer}: {other:?}").into()),
        }
    }
    Ok(())
}

#[tokio::test]
async fn private_names_leave_ip_literals_judged_as_before() -> Result<(), Box<dyn std::error::Error>> {
    assert!(matches!(
        resolve_with_private_names("10.1.2.3", &[]).await?,
        Err(ResolveError::Denied {
            class: AddressDenialClass::PrivateUse,
            ..
        })
    ));
    Ok(())
}

#[tokio::test]
async fn a_loopback_name_resolves_to_loopback_and_nothing_else() -> Result<(), Box<dyn std::error::Error>> {
    let none = AllowRanges::default();
    for (name, answer) in [
        ("localhost", "127.0.0.1"),
        ("localhost", "::1"),
        ("localhost", "::ffff:127.0.0.1"),
        ("LOCALHOST.", "127.0.0.1"),
        ("app.dev.localhost", "127.0.0.1"),
    ] {
        assert!(
            resolve_name(name, &[answer], true, &none).await.is_ok(),
            "{name} {answer}"
        );
    }
    for (name, answer) in [
        ("localhost", "169.254.169.254"),
        ("app.localhost", "192.0.2.1"),
        ("app.localhost", "10.0.0.5"),
        ("localhost", "192.0.2.10"),
    ] {
        assert!(
            matches!(
                resolve_name(name, &[answer], true, &none).await?,
                Err(ResolveError::Denied { .. })
            ),
            "{name} {answer}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn a_loopback_name_may_reach_an_allow_ranged_literal() -> Result<(), Box<dyn std::error::Error>> {
    let ranges = allow(&["10.0.0.5/32"])?;
    assert!(
        resolve_name("app.localhost", &["10.0.0.5"], true, &ranges)
            .await
            .is_ok()
    );
    Ok(())
}

#[tokio::test]
async fn names_that_only_look_like_localhost_are_judged_by_the_floor() -> Result<(), Box<dyn std::error::Error>> {
    let none = AllowRanges::default();
    for name in ["notlocalhost", "localhost.example.com"] {
        assert!(
            matches!(
                resolve_name(name, &["127.0.0.1"], true, &none).await?,
                Err(ResolveError::Denied {
                    class: AddressDenialClass::Loopback,
                    ..
                })
            ),
            "{name}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn without_the_option_a_loopback_name_stays_behind_the_floor() -> Result<(), Box<dyn std::error::Error>> {
    let none = AllowRanges::default();
    assert!(matches!(
        resolve_name("localhost", &["127.0.0.1"], false, &none).await?,
        Err(ResolveError::Denied {
            class: AddressDenialClass::Loopback,
            ..
        })
    ));
    Ok(())
}

#[tokio::test]
async fn a_public_answer_for_a_loopback_name_is_refused_even_when_the_floor_passes_it()
-> Result<(), Box<dyn std::error::Error>> {
    let none = AllowRanges::default();
    assert!(matches!(
        resolve_name("app.localhost", &["93.184.216.34"], true, &none).await?,
        Err(ResolveError::Denied {
            class: AddressDenialClass::NonLoopback,
            ..
        })
    ));
    Ok(())
}

#[tokio::test]
async fn any_non_loopback_answer_for_a_loopback_name_is_reported_as_one() -> Result<(), Box<dyn std::error::Error>> {
    let none = AllowRanges::default();
    for answer in ["169.254.169.254", "10.0.0.5", "93.184.216.34"] {
        assert!(
            matches!(
                resolve_name("app.localhost", &[answer], true, &none).await?,
                Err(ResolveError::Denied {
                    class: AddressDenialClass::NonLoopback,
                    ..
                })
            ),
            "{answer}"
        );
    }
    Ok(())
}

#[test]
fn every_floor_range_is_a_valid_cidr() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(FLOOR_NETS.len(), FLOOR_RANGES.len());
    Ok(())
}

#[test]
fn an_unresolved_destination_keeps_the_literal_the_client_named() -> Result<(), Box<dyn std::error::Error>> {
    let named = VettedAddr::unresolved(Some(ip("203.0.113.9")?));
    assert_eq!(named.ip(), ip("203.0.113.9")?);
    assert!(!named.is_vetted());
    assert_eq!(named.reported_ip(), Some(ip("203.0.113.9")?));
    Ok(())
}

#[tokio::test]
async fn the_binding_ttl_is_the_one_the_resolver_was_given() -> Result<(), Box<dyn std::error::Error>> {
    let resolver = resolver(&["93.184.216.34"])?.with_binding_ttl(std::time::Duration::from_secs(9));
    assert_eq!(resolver.bindings().ttl(), std::time::Duration::from_secs(9));
    Ok(())
}

#[tokio::test]
async fn an_address_outside_the_listed_ranges_is_judged_as_before() -> Result<(), Box<dyn std::error::Error>> {
    let outcome = resolver(&["93.184.216.34"])?
        .with_listed_addresses(listed(&["10.9.0.0/16"])?)
        .resolve(&name()?, port()?, &AllowRanges::default())
        .await;
    assert!(outcome.is_ok());
    Ok(())
}

/// Records every name it is asked, so a test can prove a name never reached DNS.
#[derive(Default)]
struct RecordingLookup(std::sync::Mutex<Vec<String>>);

impl RecordingLookup {
    fn asked(&self) -> Vec<String> {
        match self.0.lock() {
            Ok(asked) => asked.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }
}

#[async_trait]
impl NameLookup for RecordingLookup {
    async fn lookup(&self, name: &str, _port: Port) -> io::Result<Vec<IpAddr>> {
        match self.0.lock() {
            Ok(mut asked) => asked.push(name.to_string()),
            Err(poisoned) => poisoned.into_inner().push(name.to_string()),
        }
        Ok(vec![ip("203.0.113.7").map_err(|_| io::ErrorKind::InvalidData)?])
    }
}

const SENTINEL_NAMES: [&str; 3] = [
    "fake_value_6f1c2d0e-1a2b-4c3d-8e9f-0a1b2c3d4e5f.attacker.example",
    "x-FAKE_VALUE_6f1c2d0e.attacker.example",
    "eyjhbgcioijfuzi1nij9.e30.zmfrzs1zawduyxr1cmu.attacker.example",
];

#[tokio::test]
async fn a_name_carrying_a_sentinel_is_never_looked_up_to_resolve() -> Result<(), Box<dyn std::error::Error>> {
    let lookup = Arc::new(RecordingLookup::default());
    let resolver = VettingResolver::new(lookup.clone(), floor(&[])?);
    for name in SENTINEL_NAMES {
        let outcome = resolver
            .resolve_all(&Host::new(name)?, port()?, &AllowRanges::default())
            .await;
        assert!(
            matches!(outcome, Err(ResolveError::CarriesSentinel)),
            "{name}: {outcome:?}"
        );
    }
    assert!(lookup.asked().is_empty(), "looked up: {:?}", lookup.asked());
    Ok(())
}

#[tokio::test]
async fn a_name_carrying_a_sentinel_is_never_looked_up_to_confirm_a_binding() -> Result<(), Box<dyn std::error::Error>>
{
    let lookup = Arc::new(RecordingLookup::default());
    let resolver = VettingResolver::new(lookup.clone(), floor(&[])?);
    for name in SENTINEL_NAMES {
        let outcome = resolver
            .confirm_binding(&Host::new(name)?, port()?, ip("203.0.113.7")?)
            .await;
        assert!(
            matches!(outcome, Err(NameBindingError::CarriesSentinel)),
            "{name}: {outcome:?}"
        );
    }
    assert!(lookup.asked().is_empty(), "looked up: {:?}", lookup.asked());
    Ok(())
}

#[tokio::test]
async fn an_ordinary_name_is_still_looked_up() -> Result<(), Box<dyn std::error::Error>> {
    let lookup = Arc::new(RecordingLookup::default());
    let resolver = VettingResolver::new(lookup.clone(), floor(&[])?);
    let outcome = resolver
        .resolve_all(&Host::new("api.example.com")?, port()?, &AllowRanges::default())
        .await;
    assert!(outcome.is_ok(), "{outcome:?}");
    assert_eq!(lookup.asked(), vec!["api.example.com".to_string()]);
    Ok(())
}
