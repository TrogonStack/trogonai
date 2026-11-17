use super::*;

#[test]
fn digest_prefixes_the_sha256_of_its_input() {
    let fingerprint = digest("memory", "abc").unwrap();

    assert_eq!(
        fingerprint.as_str(),
        "memory:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

#[test]
fn digest_stays_bounded_for_long_inputs() {
    let fingerprint = digest("static-config", "x".repeat(1024)).unwrap();

    assert_eq!(fingerprint.as_str().len(), "static-config:".len() + 64);
}
