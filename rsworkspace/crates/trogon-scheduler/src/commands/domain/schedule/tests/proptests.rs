use super::super::RESERVED_SCHEDULE_HEADERS;
        use super::*;
        use proptest::prelude::*;

        const DOTTED_TOKEN_REGEX: &str = "[a-z][a-z0-9_-]{0,15}(\\.[a-z][a-z0-9_-]{0,15}){0,5}";

        #[derive(Debug, Clone)]
        enum DotShape {
            Leading,
            Trailing,
            Consecutive,
        }

        proptest! {
            #[test]
            fn every_duration_accepts_any_positive_duration_and_round_trips(n in 1u64..=PROTOBUF_DURATION_MAX_SECONDS) {
                let duration = Duration::from_secs(n);
                let every = EveryDuration::new(duration).unwrap();
                prop_assert_eq!(every.as_duration(), duration);
            }

            #[test]
            fn ttl_duration_accepts_any_positive_duration_and_round_trips(n in 1u64..=PROTOBUF_DURATION_MAX_SECONDS) {
                let duration = Duration::from_secs(n);
                let ttl = TtlDuration::new(duration).unwrap();
                prop_assert_eq!(ttl.as_duration(), duration);
            }

            #[test]
            fn delivery_route_accepts_any_well_formed_dotted_token(s in DOTTED_TOKEN_REGEX) {
                let route = DeliveryRoute::new(&s).unwrap();
                prop_assert_eq!(route.as_str(), s.as_str());
            }

            #[test]
            fn delivery_route_rejects_any_string_with_wildcard_or_whitespace(
                prefix in "[a-z]{1,8}",
                bad in prop_oneof![Just('*'), Just('>'), Just(' '), Just('\t'), Just('\n')],
                suffix in "[a-z]{0,8}",
            ) {
                let s = format!("{prefix}{bad}{suffix}");
                prop_assert!(DeliveryRoute::new(&s).is_err());
            }

            #[test]
            fn delivery_route_rejects_dot_boundary_violations(
                core in "[a-z]+(\\.[a-z]+){0,3}",
                shape in prop_oneof![
                    Just(DotShape::Leading),
                    Just(DotShape::Trailing),
                    Just(DotShape::Consecutive),
                ],
            ) {
                let s = match shape {
                    DotShape::Leading => format!(".{core}"),
                    DotShape::Trailing => format!("{core}."),
                    DotShape::Consecutive => format!("{core}..tail"),
                };
                prop_assert!(DeliveryRoute::new(&s).is_err());
            }

            #[test]
            fn sampling_subject_accepts_any_well_formed_dotted_token(s in DOTTED_TOKEN_REGEX) {
                let subject = SamplingSubject::new(&s).unwrap();
                prop_assert_eq!(subject.as_str(), s.as_str());
            }

            #[test]
            fn sampling_subject_rejects_any_string_with_wildcard(
                prefix in "[a-z]{1,8}",
                wildcard in prop_oneof![Just('*'), Just('>')],
                suffix in "[a-z]{0,8}",
            ) {
                let s = format!("{prefix}{wildcard}{suffix}");
                prop_assert!(SamplingSubject::new(&s).is_err());
            }

            #[test]
            fn schedule_timezone_accepts_known_iana_zones(
                s in prop_oneof![Just("UTC".to_string()), Just("America/New_York".to_string()), Just("Europe/London".to_string())],
            ) {
                let tz = ScheduleTimezone::new(&s).unwrap();
                prop_assert_eq!(tz.as_str(), s.as_str());
            }

            #[test]
            fn schedule_timezone_rejects_any_string_containing_whitespace(
                prefix in "[A-Za-z]{1,8}",
                ws in prop_oneof![Just(' '), Just('\t'), Just('\n')],
                suffix in "[A-Za-z]{0,8}",
            ) {
                let s = format!("{prefix}{ws}{suffix}");
                prop_assert!(ScheduleTimezone::new(&s).is_err());
            }

            #[test]
            fn reserved_scheduler_headers_are_rejected_in_any_case(
                name_template in proptest::sample::select(&RESERVED_SCHEDULE_HEADERS[..]),
                upper_flags in proptest::collection::vec(any::<bool>(), 32),
                value in "[ -~]{0,16}",
            ) {
                let name: String = name_template
                    .chars()
                    .zip(upper_flags.iter())
                    .map(|(ch, &upper)| if upper { ch.to_ascii_uppercase() } else { ch.to_ascii_lowercase() })
                    .collect();

                let result = ScheduleHeaders::new([(name, value)]);
                let is_reserved_error = matches!(result, Err(ScheduleHeadersError::ReservedName { .. }));
                prop_assert!(is_reserved_error);
            }

            #[test]
            fn non_reserved_headers_construct_and_preserve_input(
                name in "x-[a-z]{1,12}",
                value in "[ -~]{0,16}",
            ) {
                let headers = ScheduleHeaders::new([(name.clone(), value.clone())]).unwrap();
                let slice = headers.as_slice();
                prop_assert_eq!(slice.len(), 1);
                prop_assert_eq!(slice[0].name().as_str(), name.as_str());
                prop_assert_eq!(slice[0].value().as_str(), value.as_str());
            }

            #[test]
            fn schedule_every_constructor_matches_value_object(n in 1u64..=PROTOBUF_DURATION_MAX_SECONDS) {
                let duration = Duration::from_secs(n);
                let schedule = Schedule::every(duration).unwrap();
                match schedule {
                    Schedule::Every { every } => prop_assert_eq!(every.as_duration(), duration),
                    other => prop_assert!(false, "expected Every, got {other:?}"),
                }
            }
        }
