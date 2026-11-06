use uuid::Uuid;

use super::*;

const CHAIN: &str = "01939a4e-7b1c-7d2e-8f3a-5b6c7d8e9f01";

#[test]
fn names_a_writer_and_one_of_its_chains() {
    let writer = WriterId::new("gateway-1").unwrap();
    let chain = ChainId::from_uuid(Uuid::parse_str(CHAIN).unwrap()).unwrap();
    let key = ChainKey::new(writer.clone(), chain);

    assert_eq!(key.writer(), &writer);
    assert_eq!(key.chain(), chain);
    assert_eq!(key.to_string(), format!("gateway-1/{CHAIN}"));
}
