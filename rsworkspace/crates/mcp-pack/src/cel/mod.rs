//! Embedded Tier-2 CEL programs for the first-party pack.

pub const DEFAULT_CATALOG_FILTER: &str =
    include_str!("default_catalog_filter.cel");
pub const DEFAULT_RESOURCE_TUPLE: &str =
    include_str!("default_resource_tuple.cel");
pub const DEFAULT_AUDIT: &str = include_str!("default_audit.cel");

pub const CATALOG_FILTER_PATH: &str = "policies/default_catalog_filter.cel";
pub const RESOURCE_TUPLE_PATH: &str = "policies/default_resource_tuple.cel";
pub const AUDIT_PATH: &str = "policies/default_audit.cel";

pub const CATALOG_FILTER_ID: &str = "mcp-pack/list-tools";
pub const RESOURCE_TUPLE_ID: &str = "mcp-pack/resource-tuple";
pub const AUDIT_ID: &str = "mcp-pack/audit-default";
