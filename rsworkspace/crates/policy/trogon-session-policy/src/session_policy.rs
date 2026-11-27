use std::collections::{BTreeMap, BTreeSet};

use trogon_sandbox_contract::{DomainRule, DomainRuleAction, IsolationTier, NetworkPolicy, OpaqueTunnelPolicy};

use crate::{
    Destination, DestinationId, DestinationLocation, Limits, ModelPurpose, ModelRoute, PolicyError, PolicyFingerprint,
    PolicyInput, PrivacyMode, ReviewPolicy, ShellGrant, ToolId, WasmTool,
};

/// Immutable authority loaded and retained outside the agent's address space.
#[derive(Debug)]
pub struct SessionPolicy {
    fingerprint: PolicyFingerprint,
    mode: PrivacyMode,
    routes: BTreeMap<ModelPurpose, ModelRoute>,
    tools: BTreeMap<ToolId, WasmTool>,
    shell: Option<ShellGrant>,
    limits: Limits,
    effective_destinations: Vec<Destination>,
    privacy_destinations: Vec<Destination>,
    rule_name: trogon_policy_eval::RuleName,
}

impl SessionPolicy {
    /// Parse a bounded administrative document. Lists retain duplicate detection.
    pub fn parse(document: &str) -> Result<Self, PolicyError> {
        if document.len() > 262144 {
            return Err(PolicyError::CollectionLimit);
        }
        let input: PolicyInput = serde_json::from_str(document)?;
        Self::compile(input, PolicyFingerprint::of(document.as_bytes()))
    }

    pub fn from_input(input: PolicyInput) -> Result<Self, PolicyError> {
        let fingerprint = PolicyFingerprint::of(&serde_json::to_vec(&input)?);
        Self::compile(input, fingerprint)
    }

    fn compile(input: PolicyInput, fingerprint: PolicyFingerprint) -> Result<Self, PolicyError> {
        if input.version != 1 {
            return Err(PolicyError::Version);
        }
        if input.destinations.len() > 64
            || input.model_routes.len() > 7
            || input.execution.wasm_tools.len() > 64
            || input.privacy.allowed_destinations.len() > 64
            || input.execution.network_allowlist.len() > 64
        {
            return Err(PolicyError::CollectionLimit);
        }
        let mut destinations = BTreeMap::new();
        for value in input.destinations {
            let id = DestinationId::new(value.id)?;
            let destination = Destination::new(id.clone(), value.location, value.host, value.port)?;
            if destinations.insert(id, destination).is_some() {
                return Err(PolicyError::Duplicate);
            }
        }
        let privacy = resolve_ids(input.privacy.allowed_destinations, &destinations)?;
        if input.privacy.mode == PrivacyMode::LocalOnly
            && privacy
                .iter()
                .any(|id| destinations[id].location() != DestinationLocation::Local)
        {
            return Err(PolicyError::PrivateDestination);
        }
        let execution = resolve_ids(input.execution.network_allowlist, &destinations)?;
        let effective_destinations: Vec<_> = privacy
            .intersection(&execution)
            .map(|id| destinations[id].clone())
            .collect();
        let mut routes = BTreeMap::new();
        for value in input.model_routes {
            let id = DestinationId::new(value.destination)?;
            let destination = destinations.get(&id).ok_or(PolicyError::UnknownDestination)?;
            if !privacy.contains(&id) {
                return Err(PolicyError::PrivateDestination);
            }
            if value.model.is_empty() || value.model.len() > 200 || value.model.chars().any(char::is_control) {
                return Err(PolicyError::Identifier);
            }
            if value.purpose == ModelPurpose::Reviewer && destination.location() != DestinationLocation::Local {
                return Err(PolicyError::Reviewer);
            }
            if routes
                .insert(
                    value.purpose,
                    ModelRoute {
                        destination: destination.clone(),
                        model: value.model,
                    },
                )
                .is_some()
            {
                return Err(PolicyError::Duplicate);
            }
        }
        let mut tools = BTreeMap::new();
        for value in input.execution.wasm_tools {
            let capabilities: trogon_sandbox_contract::WasmCapabilityGrant = value.capabilities.try_into()?;
            let id = ToolId::new(value.id)?;
            if capabilities.preopens.len() > 16 || capabilities.allowed_egress.len() > 64 {
                return Err(PolicyError::CollectionLimit);
            }
            for path in &capabilities.preopens {
                if !path.host_dir().starts_with('/')
                    || path.host_dir() == "/"
                    || path.host_dir().contains('\0')
                    || path.host_dir().split('/').any(|s| s == "..")
                {
                    return Err(PolicyError::ToolCapability);
                }
            }
            for rule in &capabilities.allowed_egress {
                if !effective_destinations
                    .iter()
                    .any(|destination| destination.host() == rule.host() && destination.port() == rule.port().get())
                {
                    return Err(PolicyError::ToolCapability);
                }
            }
            if value.review == ReviewPolicy::RequiredVeto && !routes.contains_key(&ModelPurpose::Reviewer) {
                return Err(PolicyError::Reviewer);
            }
            let tool = WasmTool {
                id: id.clone(),
                component: value.component,
                capabilities,
                review: value.review,
                limits: input.execution.limits,
            };
            if tools.insert(id, tool).is_some() {
                return Err(PolicyError::Duplicate);
            }
        }
        let shell = input
            .execution
            .shell
            .map(|value| {
                if !matches!(value.isolation, IsolationTier::MicroVm | IsolationTier::FullVm) {
                    return Err(PolicyError::ShellIsolation);
                }
                if value.review == ReviewPolicy::RequiredVeto && !routes.contains_key(&ModelPurpose::Reviewer) {
                    return Err(PolicyError::Reviewer);
                }
                Ok(ShellGrant {
                    isolation: value.isolation,
                    network: network_policy(&effective_destinations),
                    limits: input.execution.limits,
                    review: value.review,
                })
            })
            .transpose()?;
        let privacy_destinations = privacy.iter().map(|id| destinations[id].clone()).collect();
        let rule_name = trogon_policy_eval::RuleName::new("session-privacy-ceiling")?;
        Ok(Self {
            privacy_destinations,
            rule_name,
            fingerprint,
            mode: input.privacy.mode,
            routes,
            tools,
            shell,
            limits: input.execution.limits,
            effective_destinations,
        })
    }

    pub(crate) fn permits_privacy_egress(&self, host: &str, port: u16) -> bool {
        self.privacy_destinations
            .iter()
            .any(|destination| destination.host() == host && destination.port() == port)
    }
    pub(crate) fn permits_model(&self, model: &str) -> bool {
        self.routes.values().any(|route| route.model() == model)
    }
    pub(crate) fn rule_name(&self) -> &trogon_policy_eval::RuleName {
        &self.rule_name
    }

    pub fn fingerprint(&self) -> PolicyFingerprint {
        self.fingerprint
    }
    pub fn privacy_mode(&self) -> PrivacyMode {
        self.mode
    }
    pub fn limits(&self) -> Limits {
        self.limits
    }
    pub fn effective_network(&self) -> NetworkPolicy {
        network_policy(&self.effective_destinations)
    }
    pub(crate) fn route(&self, purpose: ModelPurpose) -> Option<&ModelRoute> {
        self.routes.get(&purpose)
    }
    pub(crate) fn tool(&self, id: &ToolId) -> Option<&WasmTool> {
        self.tools.get(id)
    }
    pub(crate) fn shell(&self) -> Option<&ShellGrant> {
        self.shell.as_ref()
    }
}

fn resolve_ids(
    values: Vec<String>,
    destinations: &BTreeMap<DestinationId, Destination>,
) -> Result<BTreeSet<DestinationId>, PolicyError> {
    let mut ids = BTreeSet::new();
    for value in values {
        let id = DestinationId::new(value)?;
        if !destinations.contains_key(&id) {
            return Err(PolicyError::UnknownDestination);
        }
        if !ids.insert(id) {
            return Err(PolicyError::Duplicate);
        }
    }
    Ok(ids)
}

fn network_policy(destinations: &[Destination]) -> NetworkPolicy {
    if destinations.is_empty() {
        return NetworkPolicy::DenyAll;
    }
    NetworkPolicy::UserDefined {
        rules: destinations
            .iter()
            .map(|destination| DomainRule {
                domain: destination.pattern(),
                action: DomainRuleAction::PassThrough,
            })
            .collect(),
        allow_ranges: Vec::new(),
        deny_ranges: Vec::new(),
        denied_domains: Vec::new(),
        opaque_tunnels: OpaqueTunnelPolicy::Refuse,
        request_rules: Vec::new(),
    }
}
