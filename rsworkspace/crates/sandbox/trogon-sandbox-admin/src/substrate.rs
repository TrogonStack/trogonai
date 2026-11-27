//! Warming Agent Substrate actor templates ahead of the first lease.
//!
//! The substrate worker builds a template the first time it sees an image, and that build is a
//! golden snapshot that takes tens of seconds. Every lease afterwards is a create plus a resume.
//! Running this against the images a tenant actually uses moves that one-time cost off the
//! tenant's first lease, which is the only real fix for first-lease latency.

use std::sync::Arc;

use async_nats::jetstream;
use serde::Serialize;
use trogon_sandbox_protocol::default_substrate_size_classes;
use trogon_sandbox_substrate_worker::{
    ActorResources, AteError, Atespace, BearerTokenFile, ConnectSecurity, Control, DEFAULT_ROUTER_ENDPOINT,
    NatsTemplateIndex, SubstrateConfig, TemplateKey, TemplateRegistry, TlsTrust, TonicControl, validate_image_digest,
};

use crate::constants::SUBSTRATE_CONNECT_TIMEOUT;
use crate::{AdminError, Console, PrebuildRequest};

#[derive(Serialize)]
struct PrebuildReport<'a> {
    atespace: &'a str,
    template: &'a str,
    image: &'a str,
    sandbox_class: &'a str,
    size_class: &'a str,
}

/// Builds the actor template for an image and prints which template it is.
pub async fn prebuild(
    js: jetstream::Context,
    request: PrebuildRequest,
    console: &mut Console<'_>,
) -> Result<(), AdminError> {
    // The template is per atespace, so a prebuild is per tenant. Sharing one template across
    // tenants would put one tenant's golden snapshot inside another's isolation boundary.
    let atespace = Atespace::for_tenant(&request.tenant).map_err(AdminError::step("derive atespace"))?;
    let resources = ActorResources::new(request.actor_cpu_millicores, request.memory_mib)
        .map_err(AdminError::step("size actors"))?;

    let config = SubstrateConfig {
        ate_api_endpoint: request.ate_api_endpoint.clone(),
        router_endpoint: DEFAULT_ROUTER_ENDPOINT
            .parse()
            .map_err(AdminError::step("parse router endpoint"))?,
        agent_image: request.agent_image.clone(),
        agent_port: request.agent_port,
        sandbox_class: request.sandbox_class,
        snapshots_location: request.snapshots_location.clone(),
        // Unused here: this command never calls `size_selector`, because the size class it
        // builds for comes straight from `size_class_label` rather than from a memory lookup.
        // The field only exists because `SubstrateConfig` is shared with the worker.
        size_classes: default_substrate_size_classes().map_err(AdminError::step("load size classes"))?,
        template_build_timeout: request.timeout,
        resume_timeout: request.timeout,
        max_concurrent_leases: 0,
        router_tls: None,
        actor_cpu_millicores: resources.cpu_millicores(),
    };

    config
        .validate()
        .map_err(AdminError::step("validate substrate config"))?;
    validate_image_digest(&request.image).map_err(AdminError::step("validate image"))?;

    let tls = request
        .tls_ca_file
        .as_ref()
        .map(|path| TlsTrust::from_ca_file(path, request.tls_server_name.clone()))
        .transpose()
        .map_err(AdminError::step("read TLS CA file"))?;
    let bearer_token_file = Some(
        request
            .bearer_token_file
            .clone()
            .map_or_else(BearerTokenFile::default_service_account, BearerTokenFile::new),
    );
    let security = ConnectSecurity { tls, bearer_token_file };

    let control = TonicControl::connect(&request.ate_api_endpoint, SUBSTRATE_CONNECT_TIMEOUT, security)
        .await
        .map_err(AdminError::step("reach the substrate control plane"))?;

    match control.create_atespace(atespace.as_str()).await {
        Ok(_) | Err(AteError::AlreadyExists { .. }) => {}
        Err(error) => return Err(AdminError::step("create atespace")(error)),
    }

    let key = TemplateKey::new(
        &request.image,
        request.sandbox_class,
        &config,
        &request.size_class_label,
        resources,
    );

    // The same call a lease makes, so a prebuild and a cold lease cannot disagree about what
    // "ready" means or produce two different templates for one image.
    let index = NatsTemplateIndex::new(js)
        .await
        .map_err(AdminError::step("open template index"))?;
    let template_ref = TemplateRegistry::new(Arc::new(index))
        .ensure_ready(&control, &config, atespace.as_str(), &key, request.timeout)
        .await
        .map_err(AdminError::step("build template"))?;

    let report = PrebuildReport {
        atespace: &template_ref.atespace,
        template: &template_ref.name,
        image: &request.image,
        sandbox_class: request.sandbox_class.as_str(),
        size_class: &request.size_class_label,
    };
    let json = serde_json::to_string(&report).map_err(AdminError::Encode)?;
    console.out(json)
}
