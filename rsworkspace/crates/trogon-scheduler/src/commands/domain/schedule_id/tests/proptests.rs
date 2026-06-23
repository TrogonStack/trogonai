use super::*;
        use proptest::prelude::*;

        proptest! {
            #[test]
            fn accepts_any_non_empty_bounded_value(s in "[a-zA-Z0-9._:@/-]{1,256}") {
                let id = ScheduleId::parse(&s).unwrap();
                prop_assert_eq!(id.as_str(), s.as_str());
            }
        }
