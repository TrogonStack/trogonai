use super::*;
use proptest::prelude::*;

proptest! {
    #[test]
    fn accepts_uuid_bytes(bytes in any::<[u8; 16]>()) {
        let uuid = Uuid::from_bytes(bytes);
        let id = ScheduleId::parse(&uuid.hyphenated().to_string()).unwrap();
        prop_assert_eq!(id.to_string(), uuid.as_simple().to_string());
    }
}
