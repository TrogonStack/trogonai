use std::sync::Arc;

use trogon_sandbox_contract::{LeaseId, LeaseRequest, NetworkPolicy, ProviderLease, SandboxId};
use trogon_sandbox_protocol::subjects;

use crate::constants::{LEASE_TAG, NAME_TAG, SANDBOX_WORKDIR, TENANT_TAG, WAIT_ATTEMPTS, WAIT_INTERVAL};
use crate::materialized_workspace::MaterializedWorkspace;
use crate::modal_image::{ModalImage, image_env_vars};
use crate::modal_provider::ActiveModalLease;
use crate::{CpuCores, ModalProvider, ModalProviderError, ModalSandboxSpec, upload_directory};

impl ModalProvider {
    pub(crate) async fn lease_inner(
        &self,
        request: &LeaseRequest,
        lease_id: &LeaseId,
    ) -> Result<ProviderLease, ModalProviderError> {
        let gate = self.lease_gate(lease_id);
        let _held = gate.lock().await;
        if self.active.lock().await.contains_key(lease_id) {
            return Err(ModalProviderError::InvalidSpec(
                "this lease still holds a native resource; settle cleanup before reallocating".to_owned(),
            ));
        }
        reject_unsupported_mounts(request)?;
        reject_unenforceable_network_policy(request)?;
        let spec = &request.spec;
        let image = ModalImage::resolve(spec.image())?;
        let mut env: Vec<(String, String)> = spec.env().iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        let (image_name, image_id) = match &image {
            ModalImage::Registry(reference) => {
                env.extend(image_env_vars(reference));
                (reference.allowlist_string(), None)
            }
            ModalImage::Snapshot(id) => (String::new(), Some(id.clone())),
        };
        let cpu_cores = spec
            .vcpu_millis()
            .map(CpuCores::from_millis)
            .transpose()?
            .or(self.config.default_cpu_cores);
        let modal_spec = ModalSandboxSpec {
            app_name: self.config.app_name.clone(),
            image: image_name.clone(),
            image_id,
            environment: self.config.environment.clone(),
            workdir: SANDBOX_WORKDIR.to_owned(),
            timeout_seconds: request.ttl_seconds,
            cpu_cores,
            memory_mib: Some(spec.memory_mib().get()),
            region: request.region.clone(),
            env,
            tags: modal_tags(request, lease_id),
            block_network: matches!(spec.network_policy(), Some(NetworkPolicy::DenyAll)),
            outbound_cidr_allowlist: Vec::new(),
            encrypted_ports: spec.exposed_ports().iter().map(|port| port.get()).collect(),
        };
        let sandbox = self.api.create_sandbox(&modal_spec).await?;
        let (workspace, uploaded) = match self.stage(&sandbox.id, request).await {
            Ok(staged) => staged,
            Err(error) => {
                // The sandbox is already billing; a failed best-effort teardown leaves Modal's timeout to end it.
                let _ = self.api.terminate(&sandbox.id).await;
                return Err(error);
            }
        };
        let (lease, sandbox_ref) = (lease_id.as_str(), sandbox.id.as_str());
        tracing::info!(
            lease,
            sandbox = sandbox_ref,
            uploaded,
            image = image_name,
            "Modal sandbox ready"
        );
        self.active.lock().await.insert(
            lease_id.clone(),
            ActiveModalLease {
                _gate: gate.clone(),
                sandbox_id: sandbox.id.clone(),
                workspace: Arc::new(workspace),
                tenant: request.tenant.clone(),
                exposed_ports: spec.exposed_ports().to_vec(),
            },
        );
        Ok(ProviderLease {
            epoch: None,
            sandbox_id: sandbox.id,
            evidence_subject: subjects::evidence_subject(lease_id),
            worker_id: self.bound_worker(),
            attempt_id: None,
        })
    }

    /// Waits for the sandbox, then stages the workspace into it.
    async fn stage(
        &self,
        sandbox: &SandboxId,
        request: &LeaseRequest,
    ) -> Result<(MaterializedWorkspace, u64), ModalProviderError> {
        self.api.wait_running(sandbox, WAIT_ATTEMPTS, WAIT_INTERVAL).await?;
        let workspace =
            MaterializedWorkspace::materialize(request.spec.workspace(), &request.tenant, self.stores.as_ref()).await?;
        let uploaded = upload_directory(self.api.as_ref(), sandbox, SANDBOX_WORKDIR, workspace.local_path()).await?;
        Ok((workspace, uploaded))
    }
}

/// A FUSE mount lives on the worker host, and a sandbox in Modal's cloud cannot consume a host path.
fn reject_unsupported_mounts(request: &LeaseRequest) -> Result<(), ModalProviderError> {
    if request.spec.mounts().is_empty() {
        Ok(())
    } else {
        Err(ModalProviderError::InvalidSpec(
            "RemoteMount workspaces are not supported on the Modal provider: the FUSE mount is materialized on the \
             worker host, and a Modal Sandbox running in Modal's cloud cannot consume a host path"
                .to_owned(),
        ))
    }
}

/// Modal takes one switch, `block_network`. An allowlist would be served as open egress, so it is refused.
fn reject_unenforceable_network_policy(request: &LeaseRequest) -> Result<(), ModalProviderError> {
    if matches!(request.spec.network_policy(), Some(NetworkPolicy::UserDefined { .. })) {
        Err(ModalProviderError::InvalidNetworkPolicy(
            "the Modal provider can block all egress or allow it, but cannot enforce an allowlist; use DenyAll or \
             AllowAll"
                .to_owned(),
        ))
    } else {
        Ok(())
    }
}

fn modal_tags(request: &LeaseRequest, lease_id: &LeaseId) -> Vec<(String, String)> {
    let mut tags = vec![
        (LEASE_TAG.to_owned(), lease_id.as_str().to_owned()),
        (TENANT_TAG.to_owned(), request.tenant.as_str().to_owned()),
    ];
    if let Some(name) = &request.name {
        tags.push((NAME_TAG.to_owned(), name.as_str().to_owned()));
    }
    tags
}
