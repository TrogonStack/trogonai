use ipnet::IpNet;
use std::sync::LazyLock;
use trogon_sandbox_contract::AddressDenialClass;

pub(super) const FLOOR_RANGES: &[(AddressDenialClass, &str)] = &[
    (AddressDenialClass::Loopback, "127.0.0.0/8"),
    (AddressDenialClass::Loopback, "::1/128"),
    (AddressDenialClass::Unspecified, "0.0.0.0/8"),
    (AddressDenialClass::Unspecified, "::/128"),
    (AddressDenialClass::LinkLocal, "169.254.0.0/16"),
    (AddressDenialClass::LinkLocal, "fe80::/10"),
    (AddressDenialClass::CloudMetadata, "169.254.169.254/32"),
    (AddressDenialClass::CloudMetadata, "100.100.100.200/32"),
    (AddressDenialClass::CloudMetadata, "168.63.129.16/32"),
    (AddressDenialClass::CloudMetadata, "192.0.0.192/32"),
    (AddressDenialClass::CloudMetadata, "fd00:ec2::/32"),
    (AddressDenialClass::CloudMetadata, "fd20:ce::254/128"),
    (AddressDenialClass::CloudMetadata, "fd00:c1::a9fe:a9fe/128"),
    (AddressDenialClass::CloudMetadata, "fd00:42::42/128"),
    (AddressDenialClass::CloudMetadata, "fd00:a9fe:a9fe::1/128"),
    (AddressDenialClass::CloudMetadata, "fd00:100::100:200/128"),
    (AddressDenialClass::Multicast, "224.0.0.0/4"),
    (AddressDenialClass::Multicast, "ff00::/8"),
    (AddressDenialClass::Broadcast, "255.255.255.255/32"),
    (AddressDenialClass::PrivateUse, "10.0.0.0/8"),
    (AddressDenialClass::PrivateUse, "172.16.0.0/12"),
    (AddressDenialClass::PrivateUse, "192.168.0.0/16"),
    (AddressDenialClass::PrivateUse, "100.64.0.0/10"),
    (AddressDenialClass::PrivateUse, "fc00::/7"),
];

pub(super) static FLOOR_NETS: LazyLock<Vec<(AddressDenialClass, IpNet)>> = LazyLock::new(|| {
    FLOOR_RANGES
        .iter()
        .filter_map(|(class, range)| range.parse().ok().map(|net| (*class, net)))
        .collect()
});
