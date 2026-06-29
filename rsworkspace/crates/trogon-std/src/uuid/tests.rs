use super::{NowV7, UuidV7Generator};

#[test]
fn v7_generator_produces_version_7_uuids() {
    let uuid = UuidV7Generator.now_v7();
    assert_eq!(uuid.get_version_num(), 7);
}
