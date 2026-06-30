#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

#[allow(clippy::all)]
#[cfg(feature = "schedules")]
mod r#gen;

#[cfg(feature = "schedules")]
mod codec;

#[cfg(feature = "chrono")]
pub mod convert;

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

/// Failure decoding a registered event payload to canonical JSON.
#[cfg(feature = "schedules")]
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EventDecodeError {
    #[error("failed to decode '{type_url}' payload as json: {message}")]
    Json { type_url: String, message: String },
}

/// Decodes a wire-format event payload into its canonical JSON text, keyed by
/// the event's protobuf type (full name or `type.googleapis.com/` URL).
///
/// Protobuf wire encoding is not canonical, so two semantically identical
/// messages can serialize to different bytes. Comparing the decoded JSON sidesteps
/// that, which lets conformance tooling assert event equality by meaning rather
/// than by raw bytes.
///
/// Returns `Ok(None)` only for unregistered types; a registered type whose payload
/// fails to decode returns `Err`, so malformed output of a known event is never
/// mistaken for an unknown type.
#[cfg(feature = "schedules")]
pub fn decode_event_to_json(type_url: &str, payload: &[u8]) -> Result<Option<String>, EventDecodeError> {
    static REGISTRY: std::sync::OnceLock<buffa::type_registry::TypeRegistry> = std::sync::OnceLock::new();

    let registry = REGISTRY.get_or_init(|| {
        let mut registry = buffa::type_registry::TypeRegistry::new();
        r#gen::trogonai::scheduler::schedules::v1::register_types(&mut registry);
        registry
    });

    let normalized = if type_url.starts_with("type.googleapis.com/") {
        std::borrow::Cow::Borrowed(type_url)
    } else {
        std::borrow::Cow::Owned(format!("type.googleapis.com/{type_url}"))
    };

    let Some(entry) = registry.json_any_by_url(&normalized) else {
        return Ok(None);
    };
    (entry.to_json)(payload)
        .map(|value| Some(value.to_string()))
        .map_err(|message| EventDecodeError::Json {
            type_url: normalized.into_owned(),
            message,
        })
}

#[cfg(all(test, feature = "schedules"))]
mod tests;
