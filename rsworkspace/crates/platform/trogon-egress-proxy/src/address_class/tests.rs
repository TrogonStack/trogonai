use super::*;

fn class(raw: &str) -> AddressClass {
    classify(raw.parse().expect("test address"))
}

#[test]
fn ipv4_ranges() {
    let cases = [
        ("127.0.0.1", AddressClass::Loopback),
        ("127.255.255.254", AddressClass::Loopback),
        ("0.0.0.0", AddressClass::Unspecified),
        ("0.1.2.3", AddressClass::Unspecified),
        ("169.254.169.254", AddressClass::LinkLocal),
        ("169.254.0.1", AddressClass::LinkLocal),
        ("10.0.0.1", AddressClass::Private),
        ("10.255.255.255", AddressClass::Private),
        ("172.16.0.1", AddressClass::Private),
        ("172.31.255.255", AddressClass::Private),
        ("192.168.1.1", AddressClass::Private),
        ("100.64.0.1", AddressClass::Cgnat),
        ("100.127.255.255", AddressClass::Cgnat),
        ("224.0.0.1", AddressClass::Multicast),
        ("239.255.255.255", AddressClass::Multicast),
        ("240.0.0.1", AddressClass::Reserved),
        ("255.255.255.255", AddressClass::Reserved),
    ];
    for (address, expected) in cases {
        assert_eq!(class(address), expected, "{address}");
    }
}

#[test]
fn ipv4_range_boundaries_are_public() {
    for address in [
        "1.1.1.1",
        "8.8.8.8",
        "100.63.255.255",
        "100.128.0.1",
        "172.15.255.255",
        "172.32.0.1",
        "169.253.0.1",
        "192.167.0.1",
        "192.169.0.1",
        "11.0.0.1",
        "126.255.255.255",
        "128.0.0.1",
        "223.255.255.255",
    ] {
        assert_eq!(class(address), AddressClass::Public, "{address}");
    }
}

#[test]
fn ipv6_ranges() {
    let cases = [
        ("::1", AddressClass::Loopback),
        ("::", AddressClass::Unspecified),
        ("fe80::1", AddressClass::LinkLocal),
        ("febf::1", AddressClass::LinkLocal),
        ("fc00::1", AddressClass::UniqueLocal),
        ("fd12:3456:789a::1", AddressClass::UniqueLocal),
        ("fec0::1", AddressClass::Private),
        ("ff02::1", AddressClass::Multicast),
    ];
    for (address, expected) in cases {
        assert_eq!(class(address), expected, "{address}");
    }
}

#[test]
fn ipv6_public_addresses_and_boundaries() {
    for address in ["2001:4860:4860::8888", "2606:4700::1111", "fbff::1", "fe7f::1"] {
        assert_eq!(class(address), AddressClass::Public, "{address}");
    }
}

#[test]
fn ipv4_embedded_in_ipv6_takes_the_ipv4_class() {
    let cases = [
        ("::ffff:127.0.0.1", AddressClass::Loopback),
        ("::ffff:169.254.169.254", AddressClass::LinkLocal),
        ("::ffff:10.1.2.3", AddressClass::Private),
        ("::ffff:100.64.0.1", AddressClass::Cgnat),
        ("::ffff:8.8.8.8", AddressClass::Public),
        ("::127.0.0.1", AddressClass::Loopback),
        ("::10.0.0.1", AddressClass::Private),
        ("64:ff9b::7f00:1", AddressClass::Loopback),
        ("64:ff9b::a9fe:a9fe", AddressClass::LinkLocal),
        ("64:ff9b::808:808", AddressClass::Public),
        ("2002:7f00:1::1", AddressClass::Loopback),
        ("2002:a00:1::1", AddressClass::Private),
        ("2002:808:808::1", AddressClass::Public),
    ];
    for (address, expected) in cases {
        assert_eq!(class(address), expected, "{address}");
    }
}

#[test]
fn only_public_is_public() {
    assert!(AddressClass::Public.is_public());
    for class in [
        AddressClass::Loopback,
        AddressClass::LinkLocal,
        AddressClass::Private,
        AddressClass::Cgnat,
        AddressClass::UniqueLocal,
        AddressClass::Unspecified,
        AddressClass::Multicast,
        AddressClass::Reserved,
    ] {
        assert!(!class.is_public());
    }
}
