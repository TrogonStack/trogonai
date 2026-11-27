//! Injective NATS KV keys for records scoped to a tenant and a name.
//!
//! The legacy layout, `{tenant}_{name}` with `:`, `/` and `.` folded to `_`,
//! is not injective: tenant `acme` with name `team_prod` and tenant
//! `acme_team` with name `prod` share one key, and so do names that differ
//! only in a folded character. The current layout escapes both parts so the
//! tenant part never contains the `_` separator:
//!
//! - tenant: `[A-Za-z0-9-]` kept, every other byte written as `=HH`
//! - name: `[A-Za-z0-9_-]` kept, every other byte written as `=HH`
//!
//! The first `_` of a key therefore always ends the tenant part, and both
//! escapes decode uniquely, so distinct `(tenant, name)` pairs never share a
//! key. For the common case (tenant in `[A-Za-z0-9-]`, name in
//! `[A-Za-z0-9_-]`) the current key equals the legacy key, so existing
//! records stay where they are; the legacy helpers below exist so stores can
//! still find records written under a legacy key that differs.

use crate::constants::{ESCAPE, SEPARATOR};
use trogon_sandbox_contract::TenantId;

fn escape_into(out: &mut String, value: &str, keep_underscore: bool) {
    for byte in value.bytes() {
        let kept = byte.is_ascii_alphanumeric() || byte == b'-' || (keep_underscore && byte == b'_');
        if kept {
            out.push(char::from(byte));
        } else {
            out.push(char::from(ESCAPE));
            out.push_str(&format!("{byte:02X}"));
        }
    }
}

fn unescape(value: &str, allow_underscore: bool) -> Option<String> {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while let Some(&byte) = bytes.get(index) {
        if byte == ESCAPE {
            let hex = value.get(index + 1..index + 3)?;
            let decoded = u8::from_str_radix(hex, 16).ok()?;
            // Only the canonical (uppercase, escaped-only-when-needed) form
            // decodes, so every decoded name maps back to exactly this key.
            let canonical =
                !(decoded.is_ascii_alphanumeric() || decoded == b'-' || (allow_underscore && decoded == b'_'));
            if !canonical || hex.bytes().any(|b| b.is_ascii_lowercase()) {
                return None;
            }
            out.push(decoded);
            index += 3;
        } else if byte.is_ascii_alphanumeric() || byte == b'-' || (allow_underscore && byte == b'_') {
            out.push(byte);
            index += 1;
        } else {
            return None;
        }
    }
    String::from_utf8(out).ok()
}

/// Key prefix shared by every current-layout key of `tenant`. A key that
/// starts with it belongs to `tenant` and no other tenant.
pub fn tenant_scoped_kv_prefix(tenant: &TenantId) -> String {
    let mut out = String::with_capacity(tenant.as_str().len() + 1);
    escape_into(&mut out, tenant.as_str(), false);
    out.push(SEPARATOR);
    out
}

/// Current-layout key for `name` under `tenant`; injective over both parts.
pub fn tenant_scoped_kv_key(tenant: &TenantId, name: &str) -> String {
    let mut out = tenant_scoped_kv_prefix(tenant);
    escape_into(&mut out, name, true);
    out
}

/// Recovers the name from a current-layout key of `tenant`, or `None` when
/// the key belongs to another tenant or is not a canonical current key.
pub fn parse_tenant_scoped_kv_key(key: &str, tenant: &TenantId) -> Option<String> {
    let rest = key.strip_prefix(&tenant_scoped_kv_prefix(tenant))?;
    unescape(rest, true)
}

/// Recovers the tenant from a current-layout key written by
/// [`tenant_scoped_kv_key`], without knowing the tenant in advance. This is
/// what lets a sweeper enumerate tenants from a bucket instead of a fixed
/// list. `None` for a key that is not canonical (a legacy key whose tenant
/// held a folded character, or a key with no separator) and for a key whose
/// tenant part does not decode to a valid [`TenantId`].
pub fn parse_tenant_scoped_kv_key_any(key: &str) -> Option<(TenantId, String)> {
    let (tenant, name) = key.split_once(SEPARATOR)?;
    if tenant.is_empty() {
        return None;
    }
    let tenant = TenantId::new(unescape(tenant, false)?).ok()?;
    let name = unescape(name, true)?;
    Some((tenant, name))
}

/// Current-layout key for a record scoped to `tenant` alone, such as a
/// tenant's limits or allow-lists. Injective: tenants `a.b` and `a_b`, which
/// shared the legacy key `a_b`, get `a=2Eb` and `a=5Fb`.
pub fn tenant_kv_key(tenant: &TenantId) -> String {
    let mut out = String::with_capacity(tenant.as_str().len());
    escape_into(&mut out, tenant.as_str(), false);
    out
}

/// Legacy (non-injective) key of a record scoped to `tenant` alone, kept
/// only to find records written before the current layout.
pub fn legacy_tenant_kv_key(tenant: &TenantId) -> String {
    tenant.as_str().replace([':', '/', '.'], "_")
}

/// Legacy (non-injective) key layout, kept only to find records written
/// before the current layout. Never write new records under it.
pub fn legacy_tenant_scoped_kv_key(tenant: &TenantId, name: &str) -> String {
    format!(
        "{}{}",
        legacy_tenant_scoped_kv_prefix(tenant),
        name.replace([':', '/', '.'], "_")
    )
}

/// Legacy key prefix of `tenant`. Unlike the current prefix, keys of other
/// tenants can start with it (tenant `acme_team` under prefix `acme_`).
pub fn legacy_tenant_scoped_kv_prefix(tenant: &TenantId) -> String {
    format!("{}_", tenant.as_str().replace([':', '/', '.'], "_"))
}
