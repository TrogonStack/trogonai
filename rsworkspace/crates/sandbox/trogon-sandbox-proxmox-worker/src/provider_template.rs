use serde_json::Value;

use crate::ProxmoxProviderError;
use crate::check::ensure;

fn numbered(name: &str, prefix: &str) -> bool {
    name.strip_prefix(prefix)
        .is_some_and(|suffix| !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit()))
}

/// Refuses a template that is not one, or that would hand its clones passthrough devices, shared memory or
/// execution hooks.
pub(crate) fn ensure_clonable(template: &Value) -> Result<(), ProxmoxProviderError> {
    ensure(
        template["template"].as_u64() == Some(1),
        ProxmoxProviderError::Rejected("the configured VM is not a template"),
    )?;
    let config = template
        .as_object()
        .ok_or(ProxmoxProviderError::Backend("invalid template config"))?;
    for (name, value) in config {
        if value.is_null() || value.as_str() == Some("") {
            continue;
        }
        let unsupported = matches!(name.as_str(), "args" | "ivshmem" | "hookscript")
            || ["hostpci", "usb", "virtiofs", "parallel"]
                .iter()
                .any(|prefix| numbered(name, prefix))
            || (numbered(name, "serial") && value.as_str() != Some("socket"));
        ensure(
            !unsupported,
            ProxmoxProviderError::Rejected(
                "template contains unsupported passthrough, shared memory, or execution hooks",
            ),
        )?;
    }
    Ok(())
}

/// The names of the NICs a clone inherited.
pub(crate) fn nic_keys(config: &Value) -> Result<Vec<String>, ProxmoxProviderError> {
    let config = config
        .as_object()
        .ok_or(ProxmoxProviderError::Backend("invalid VM config"))?;
    Ok(config.keys().filter(|key| numbered(key, "net")).cloned().collect())
}
