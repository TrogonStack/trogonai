//! CEL policy → Envoy RBAC filter mapping.
//!
//! Statically translatable patterns become inline RBAC rules. Everything else
//! falls back to an `ext_authz` gRPC reference (Trogon gateway policy evaluation).

use std::collections::HashMap;

use prost::Message;
use prost_types::Any;

use crate::config::{PolicyEffect, PolicyRule};
use crate::proto::envoy::config::core::v3::GrpcService;
use crate::proto::envoy::config::rbac::v3::permission;
use crate::proto::envoy::config::rbac::v3::rbac;
use crate::proto::envoy::config::rbac::v3::{Permission, Policy, Principal, Rbac as RbacPolicy};
use crate::proto::envoy::extensions::filters::http::ext_authz::v3::ext_authz;
use crate::proto::envoy::extensions::filters::http::ext_authz::v3::ExtAuthz;
use crate::proto::envoy::extensions::filters::http::rbac::v3::Rbac as HttpRbacFilter;
use crate::proto::envoy::extensions::filters::network::http_connection_manager::v3::http_filter;
use crate::proto::envoy::r#type::matcher::v3::path_matcher;
use crate::proto::envoy::r#type::matcher::v3::string_matcher;
use crate::proto::envoy::r#type::matcher::v3::{PathMatcher, StringMatcher};
use crate::type_urls;

#[derive(Clone, Debug, Default)]
pub struct RbacMapping {
    pub filters: Vec<(String, HttpRbacFilter)>,
    pub requires_ext_authz: bool,
    pub untranslatable: Vec<String>,
}

pub fn map_policies(policies: &[PolicyRule]) -> RbacMapping {
    let mut allow_permissions = Vec::new();
    let mut deny_permissions = Vec::new();
    let mut untranslatable = Vec::new();
    let mut requires_ext_authz = false;

    for policy in policies {
        match translate_cel(policy) {
            CelTranslation::MethodEq { method, effect } => {
                let permission = Permission {
                    rule: Some(permission::Rule::UrlPath(PathMatcher {
                        rule: Some(path_matcher::Rule::Path(StringMatcher {
                            match_pattern: Some(string_matcher::MatchPattern::Prefix(
                                format!("/{method}"),
                            )),
                            ..Default::default()
                        })),
                    })),
                };
                match effect {
                    PolicyEffect::Allow => allow_permissions.push(permission),
                    PolicyEffect::Deny => deny_permissions.push(permission),
                }
            }
            CelTranslation::HeaderEq { header, value, effect } => {
                let permission = Permission {
                    rule: Some(permission::Rule::Header(
                        crate::proto::envoy::config::route::v3::HeaderMatcher {
                            name: header,
                            header_match_specifier: Some(
                                crate::proto::envoy::config::route::v3::header_matcher::HeaderMatchSpecifier::StringMatch(
                                    StringMatcher {
                                        match_pattern: Some(string_matcher::MatchPattern::Exact(
                                            value,
                                        )),
                                        ..Default::default()
                                    },
                                ),
                            ),
                            ..Default::default()
                        },
                    )),
                };
                match effect {
                    PolicyEffect::Allow => allow_permissions.push(permission),
                    PolicyEffect::Deny => deny_permissions.push(permission),
                }
            }
            CelTranslation::Untranslatable => {
                untranslatable.push(policy.name.clone());
                requires_ext_authz = true;
            }
        }
    }

    let mut policies_map = HashMap::new();
    if !allow_permissions.is_empty() {
        policies_map.insert(
            "allow-policy".into(),
            Policy {
                permissions: allow_permissions,
                principals: vec![Principal {
                    identifier: Some(
                        crate::proto::envoy::config::rbac::v3::principal::Identifier::Any(true),
                    ),
                    ..Default::default()
                }],
                ..Default::default()
            },
        );
    }
    if !deny_permissions.is_empty() {
        policies_map.insert(
            "deny-policy".into(),
            Policy {
                permissions: deny_permissions,
                principals: vec![Principal {
                    identifier: Some(
                        crate::proto::envoy::config::rbac::v3::principal::Identifier::Any(true),
                    ),
                    ..Default::default()
                }],
                ..Default::default()
            },
        );
    }

    let action = if policies_map.is_empty() {
        rbac::Action::Allow as i32
    } else {
        rbac::Action::Allow as i32
    };

    let http_rbac = HttpRbacFilter {
        rules: Some(RbacPolicy {
            action,
            policies: policies_map,
            ..Default::default()
        }),
        ..Default::default()
    };

    RbacMapping {
        filters: vec![("trogon-rbac".into(), http_rbac)],
        requires_ext_authz,
        untranslatable,
    }
}

enum CelTranslation {
    MethodEq {
        method: String,
        effect: PolicyEffect,
    },
    HeaderEq {
        header: String,
        value: String,
        effect: PolicyEffect,
    },
    Untranslatable,
}

fn translate_cel(policy: &PolicyRule) -> CelTranslation {
    let cel = policy.cel.trim();
    if let Some(method) = parse_method_eq(cel) {
        return CelTranslation::MethodEq {
            method,
            effect: policy.effect,
        };
    }
    if let Some((header, value)) = parse_header_eq(cel) {
        return CelTranslation::HeaderEq {
            header,
            value,
            effect: policy.effect,
        };
    }
    CelTranslation::Untranslatable
}

fn parse_method_eq(cel: &str) -> Option<String> {
    const PREFIX: &str = "mcp.method == \"";
    if !cel.starts_with(PREFIX) {
        return None;
    }
    let rest = &cel[PREFIX.len()..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

fn parse_header_eq(cel: &str) -> Option<(String, String)> {
    const PREFIX: &str = "request.headers[";
    if !cel.starts_with(PREFIX) {
        return None;
    }
    let rest = &cel[PREFIX.len()..];
    let quote_end = rest.find('"')?;
    let header = rest[..quote_end].to_string();
    let after_header = &rest[quote_end + 1..];
    if !after_header.starts_with("] == \"") {
        return None;
    }
    let value_part = &after_header["] == \"".len()..];
    let value_end = value_part.find('"')?;
    Some((header, value_part[..value_end].to_string()))
}

impl RbacMapping {
    pub fn http_filter(&self) -> Option<crate::proto::envoy::extensions::filters::network::http_connection_manager::v3::HttpFilter> {
        let rbac = self.filters.first()?.1.clone();
        Some(crate::proto::envoy::extensions::filters::network::http_connection_manager::v3::HttpFilter {
            name: "envoy.filters.http.rbac".into(),
            config_type: Some(http_filter::ConfigType::TypedConfig(Any {
                type_url: type_urls::RBAC.into(),
                value: rbac.encode_to_vec(),
            })),
            ..Default::default()
        })
    }

    pub fn ext_authz_filter(&self) -> Option<crate::proto::envoy::extensions::filters::network::http_connection_manager::v3::HttpFilter> {
        if !self.requires_ext_authz {
            return None;
        }
        let ext_authz = ExtAuthz {
            services: Some(ext_authz::Services::GrpcService(GrpcService {
                target_specifier: Some(
                    crate::proto::envoy::config::core::v3::grpc_service::TargetSpecifier::EnvoyGrpc(
                        crate::proto::envoy::config::core::v3::grpc_service::EnvoyGrpc {
                            cluster_name: "trogon_ext_authz".into(),
                            ..Default::default()
                        },
                    ),
                ),
                ..Default::default()
            })),
            ..Default::default()
        };
        Some(crate::proto::envoy::extensions::filters::network::http_connection_manager::v3::HttpFilter {
            name: "envoy.filters.http.ext_authz".into(),
            config_type: Some(http_filter::ConfigType::TypedConfig(Any {
                type_url:
                    "type.googleapis.com/envoy.extensions.filters.http.ext_authz.v3.ExtAuthz"
                        .into(),
                value: ext_authz.encode_to_vec(),
            })),
            ..Default::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{PolicyEffect, PolicyRule};

    #[test]
    fn rbac_maps_method_equality_cel() {
        let policies = vec![PolicyRule {
            name: "tools-call".into(),
            cel: r#"mcp.method == "tools/call""#.into(),
            effect: PolicyEffect::Allow,
        }];
        let mapping = map_policies(&policies);
        assert!(!mapping.requires_ext_authz);
        assert_eq!(mapping.filters.len(), 1);
    }

    #[test]
    fn rbac_falls_back_to_ext_authz_for_complex_cel() {
        let policies = vec![PolicyRule {
            name: "admin".into(),
            cel: r#"agent.purpose == "admin""#.into(),
            effect: PolicyEffect::Deny,
        }];
        let mapping = map_policies(&policies);
        assert!(mapping.requires_ext_authz);
        assert_eq!(mapping.untranslatable, vec!["admin".to_string()]);
        assert!(mapping.ext_authz_filter().is_some());
    }
}
