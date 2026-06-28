#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

#[allow(clippy::all)]
#[cfg(any(feature = "schedules", feature = "light"))]
mod r#gen;

#[cfg(any(feature = "schedules", feature = "light"))]
mod codec;

#[cfg(feature = "chrono")]
pub mod convert;

#[cfg(feature = "light")]
pub mod example;

#[cfg(feature = "schedules")]
pub mod scheduler;

// Thin wrappers that re-export the generated proto packages, emitted as inline
// module trees that mirror the codegen layout.
#[cfg(feature = "schedules")]
#[cfg_attr(dylint_lib = "trogon_lints", allow(inline_module_block))]
pub mod content {
    pub mod v1alpha1 {
        pub use crate::r#gen::trogon::content::v1alpha1::*;
    }
}

#[cfg(feature = "schedules")]
#[cfg_attr(dylint_lib = "trogon_lints", allow(inline_module_block))]
pub mod google {
    pub mod r#type {
        pub use crate::r#gen::google::r#type::*;
    }
}

/// Decodes a wire-format event payload into its canonical JSON text, keyed by
/// the event's protobuf type (full name or `type.googleapis.com/` URL).
///
/// Protobuf wire encoding is not canonical, so two semantically identical
/// messages can serialize to different bytes. Comparing the decoded JSON sidesteps
/// that, which lets conformance tooling assert event equality by meaning rather
/// than by raw bytes. Returns `None` for unregistered types.
#[cfg(any(feature = "schedules", feature = "light"))]
pub fn decode_event_to_json(type_url: &str, payload: &[u8]) -> Option<String> {
    use std::sync::OnceLock;

    static REGISTRY: OnceLock<buffa::type_registry::TypeRegistry> = OnceLock::new();

    let registry = REGISTRY.get_or_init(|| {
        let mut registry = buffa::type_registry::TypeRegistry::new();
        #[cfg(feature = "light")]
        r#gen::trogonai::example::light::v1::register_types(&mut registry);
        #[cfg(feature = "schedules")]
        r#gen::trogonai::scheduler::schedules::v1::register_types(&mut registry);
        registry
    });

    let normalized = if type_url.starts_with("type.googleapis.com/") {
        std::borrow::Cow::Borrowed(type_url)
    } else {
        std::borrow::Cow::Owned(format!("type.googleapis.com/{type_url}"))
    };

    let entry = registry.json_any_by_url(&normalized)?;
    (entry.to_json)(payload).ok().map(|value| value.to_string())
}

#[cfg(all(test, feature = "light"))]
mod tests {
    use buffa::{Message as _, MessageName as _};

    use crate::example::v1::LightTurnedOn;

    #[test]
    fn decode_event_to_json_is_canonical_across_wire_orderings() {
        let event = LightTurnedOn {
            light_id: "kitchen".to_string(),
            turn_on_count: 3,
        };
        let canonical = event.encode_to_vec();

        // The same message with fields written in reverse wire order (field 2
        // before field 1). Protobuf permits any field order, so this is a valid
        // alternate encoding that differs on the wire.
        let reordered = {
            let mut bytes = Vec::new();
            bytes.extend_from_slice(&[0x10, 0x03]); // field 2 (turn_on_count) varint = 3
            bytes.push(0x0A); // field 1 (light_id) length-delimited
            bytes.push(7);
            bytes.extend_from_slice(b"kitchen");
            bytes
        };
        assert_ne!(canonical, reordered, "encodings must differ on the wire");

        let from_canonical = super::decode_event_to_json(LightTurnedOn::FULL_NAME, &canonical);
        let from_reordered = super::decode_event_to_json(LightTurnedOn::FULL_NAME, &reordered);

        assert!(from_canonical.is_some());
        assert_eq!(from_canonical, from_reordered);
    }

    #[test]
    fn decode_event_to_json_returns_none_for_unknown_type() {
        assert_eq!(
            super::decode_event_to_json("type.googleapis.com/trogonai.example.light.v1.Unknown", &[]),
            None
        );
    }
}
