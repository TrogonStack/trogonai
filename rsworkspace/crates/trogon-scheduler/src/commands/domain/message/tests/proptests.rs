use super::*;
        use proptest::prelude::*;

        proptest! {
            #[test]
            fn message_headers_accept_valid_name_value_pairs(
                name in "x-[a-z]{1,16}",
                value in "[ -~]{0,32}",
            ) {
                let headers = MessageHeaders::new([(name.as_str(), value.as_str())]).unwrap();
                let slice = headers.as_slice();
                prop_assert_eq!(slice.len(), 1);
                prop_assert_eq!(slice[0].name().as_str(), name.as_str());
                prop_assert_eq!(slice[0].value().as_str(), value.as_str());
            }

            #[test]
            fn message_headers_reject_name_containing_colon(
                prefix in "[a-z]{1,8}",
                suffix in "[a-z]{0,8}",
                value in "[ -~]{0,16}",
            ) {
                let name = format!("{prefix}:{suffix}");
                prop_assert!(MessageHeaders::new([(name, value)]).is_err());
            }

            #[test]
            fn message_headers_reject_name_containing_whitespace_or_control(
                prefix in "[a-z]{1,8}",
                bad in prop_oneof![Just(' '), Just('\t'), Just('\n'), Just('\r'), Just('\0')],
                suffix in "[a-z]{0,8}",
                value in "[ -~]{0,16}",
            ) {
                let name = format!("{prefix}{bad}{suffix}");
                prop_assert!(MessageHeaders::new([(name, value)]).is_err());
            }

            #[test]
            fn message_headers_reject_value_containing_crlf_or_null(
                name in "x-[a-z]{1,8}",
                prefix in "[ -~]{0,8}",
                bad in prop_oneof![Just('\r'), Just('\n'), Just('\0')],
                suffix in "[ -~]{0,8}",
            ) {
                let value = format!("{prefix}{bad}{suffix}");
                prop_assert!(MessageHeaders::new([(name, value)]).is_err());
            }
        }
