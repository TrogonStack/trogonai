//! Event metadata headers.
//!
//! Headers are metadata attached to the event envelope, not part of the domain
//! payload. They are intentionally small value objects so invalid names and
//! transport-hostile values are rejected before adapters persist them.

mod from_entries_error;
mod header_map;
mod header_name;
mod header_value;

pub use from_entries_error::FromEntriesError;
pub use header_map::Headers;
pub use header_name::{HeaderName, HeaderNameError};
pub use header_value::{HeaderValue, HeaderValueError};

#[cfg(test)]
mod tests {
    use super::*;
    use std::{borrow::Borrow, error::Error as _, str::FromStr};

    #[test]
    fn header_names_cover_conversions_display_and_borrowing() {
        let name = HeaderName::new("trace-id").unwrap();

        assert_eq!(name.as_str(), "trace-id");
        assert_eq!(HeaderName::from_str("trace-id").unwrap(), name);
        assert_eq!(HeaderName::try_from("trace-id"), Ok(name.clone()));
        assert_eq!(HeaderName::try_from("trace-id".to_string()), Ok(name.clone()));
        assert_eq!(name.to_string(), "trace-id");
        assert_eq!(<HeaderName as Borrow<str>>::borrow(&name), "trace-id");

        let empty_error = HeaderName::new("").unwrap_err();
        assert_eq!(empty_error, HeaderNameError::Empty);
        assert_eq!(empty_error.to_string(), "event header name cannot be empty");
        let _: &dyn std::error::Error = &empty_error;

        let control_error = HeaderName::new("trace\nid").unwrap_err();
        assert_eq!(control_error, HeaderNameError::ContainsControlCharacter);
        assert_eq!(
            control_error.to_string(),
            "event header name cannot contain control characters"
        );
        let _: &dyn std::error::Error = &control_error;
    }

    #[test]
    fn header_values_cover_conversions_display_and_refs() {
        let value = HeaderValue::new("trace-1").unwrap();

        assert_eq!(value.as_str(), "trace-1");
        assert_eq!(HeaderValue::from_str("trace-1").unwrap(), value);
        assert_eq!(HeaderValue::try_from("trace-1"), Ok(value.clone()));
        assert_eq!(HeaderValue::try_from("trace-1".to_string()), Ok(value.clone()));
        assert_eq!(HeaderValue::try_from(&"trace-1".to_string()), Ok(value.clone()));
        assert_eq!(value.to_string(), "trace-1");
        assert_eq!(AsRef::<str>::as_ref(&value), "trace-1");
        assert_eq!(AsRef::<[u8]>::as_ref(&value), b"trace-1");
        assert_eq!(value.clone().into_string(), "trace-1");

        let error = HeaderValue::new("line\nbreak").unwrap_err();
        assert_eq!(
            error.to_string(),
            "event header value cannot contain '\\r', '\\n', or '\\0'"
        );
        let _: &dyn std::error::Error = &error;
    }

    #[test]
    fn headers_preserve_deterministic_order_and_replace_values() {
        let mut headers = Headers::empty();

        assert!(headers.is_empty());
        assert_eq!(headers.len(), 0);

        headers.insert(HeaderName::new("zeta").unwrap(), "last").unwrap();
        headers.insert(HeaderName::new("alpha").unwrap(), "first").unwrap();
        let previous = headers.insert(HeaderName::new("zeta").unwrap(), "updated").unwrap();

        assert_eq!(previous.map(HeaderValue::into_string), Some("last".to_string()));
        assert_eq!(headers.len(), 2);
        assert_eq!(headers.get_str("zeta"), Some("updated"));
        assert_eq!(headers.get("missing"), None);

        let names = headers
            .iter()
            .map(|(name, value)| (name.as_str().to_string(), value.as_str().to_string()))
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            vec![
                ("alpha".to_string(), "first".to_string()),
                ("zeta".to_string(), "updated".to_string())
            ]
        );
    }

    #[test]
    fn headers_from_entries_reports_name_and_value_sources() {
        let headers = Headers::from_entries([("trace-id", "trace-1"), ("request-id", "req-1")]).unwrap();

        assert_eq!(headers.len(), 2);
        assert_eq!(headers.get_str("trace-id"), Some("trace-1"));

        let name_error = Headers::from_entries([("", "value")]).unwrap_err();
        assert_eq!(name_error.to_string(), "event header name '' is not valid");
        assert!(name_error.source().is_some());
        assert!(matches!(name_error, FromEntriesError::InvalidName { .. }));

        let control_name_error = Headers::from_entries([("trace\nid", "value")]).unwrap_err();
        assert_eq!(
            control_name_error.to_string(),
            "event header name 'trace\nid' is not valid"
        );
        assert!(control_name_error.source().is_some());
        assert!(matches!(
            control_name_error,
            FromEntriesError::InvalidName {
                source: HeaderNameError::ContainsControlCharacter,
                ..
            }
        ));

        let value_error = Headers::from_entries([("trace-id", "line\rbreak")]).unwrap_err();
        assert_eq!(
            value_error.to_string(),
            "event header value for 'trace-id' is not valid"
        );
        assert!(value_error.source().is_some());
        assert!(matches!(value_error, FromEntriesError::InvalidValue { .. }));
    }
}
