use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use async_trait::async_trait;
use k8s_openapi::api::batch::v1::{Job, JobSpec};
use k8s_openapi::api::core::v1::{
    ConfigMap, Container, EnvVar, PodSpec, PodTemplateSpec, ResourceRequirements, SecretVolumeSource, SecurityContext,
    Volume, VolumeMount,
};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{ObjectMeta, OwnerReference};
use tokio::sync::watch;
use trogon_sandbox_contract::{CloneAuth, DomainRule, DomainRuleAction, LeaseId, NetworkPolicy, SandboxPhase};
use trogon_sandbox_protocol::{ca_trust_env, run_phase};

use crate::k8s_microvm_handle::K8sMicroVmHandle;
use crate::k8s_network_policy::EGRESS_PROXY_CONFIGMAP_KEY;
use crate::k8s_ops::constants::{
    EVIDENCE_PATH, LABEL_DIGEST, LABEL_LEASE_ID, LABEL_TENANT_ID, LABEL_WORKER_IDENTITY, RESTORE_VOLUME_NAME,
    RUNTIME_TMP_VOLUME, SANDBOX_TERMINATION_GRACE_SECONDS, WORKSPACE_PATH,
};
use crate::k8s_ops::{
    clone_secret_name, container_resources, hardened_pod_security_context, is_strong_isolation_runtime_class,
    job_deadline_seconds, keep_alive_command, parse_run_as_user, runtime_class_name, runtime_tmp_volume,
};
use crate::{
    EGRESS_PROXY_CA_MOUNT_PATH, EGRESS_PROXY_CA_VOLUME_NAME, EgressPolicy, EgressViolationEvents, K8sError,
    K8sLaunchOptions, K8sMicroVmProvider, K8sProviderError, K8sResourceKind, K8sResourceReceipt, SandboxError,
    SandboxHandle, SandboxProvider, SandboxSpec, SandboxStatus, build_clone_secret, build_egress_sidecar,
    build_git_init_container, build_network_policy_manifest, build_workspace_import_init_container, digest_label_value,
    egress_proxy_configmap_name, job_name, network_policy_name, policy_needs_egress_proxy, reject_proxy_uid_workload,
    resolve_secret_bytes,
};

#[async_trait]
impl SandboxProvider for K8sMicroVmProvider {
    async fn launch(&self, spec: SandboxSpec) -> Result<Box<dyn SandboxHandle>, SandboxError> {
        Ok(self
            .launch_with_options(spec, K8sLaunchOptions::default())
            .await
            .map_err(K8sError::from)?)
    }
}

impl K8sMicroVmProvider {
    pub async fn launch_with_options(
        &self,
        spec: SandboxSpec,
        opts: K8sLaunchOptions,
    ) -> Result<Box<dyn SandboxHandle>, K8sProviderError> {
        if spec.host_mount.is_some() {
            return Err(K8sProviderError::HostMountUnsupported);
        }

        let run_as_ids = spec.user.as_deref().map(parse_run_as_user).transpose()?;

        let emitter = self.lifecycle.as_deref();

        let repo = &spec.repo;
        let job_name = job_name(&repo.owner, &repo.name, &spec.resolved_sha);
        let provider_str = repo.provider.as_str().to_string();

        let tools_csv = spec
            .tools
            .iter()
            .map(|t| t.selector.0.as_str())
            .collect::<Vec<_>>()
            .join(",");

        let mut labels = BTreeMap::new();
        labels.insert(
            "app.kubernetes.io/name".to_string(),
            self.config.resource_naming.agent_name().to_string(),
        );
        labels.insert(
            self.config.resource_naming.repo_label().to_string(),
            format!("{}-{}-{}", provider_str, repo.owner, repo.name),
        );
        if matches!(spec.egress_policy, EgressPolicy::DenyAll) {
            labels.insert(
                self.config.resource_naming.egress_label().to_string(),
                "deny-all".to_string(),
            );
        }
        labels.insert(self.config.resource_naming.job_label().to_string(), job_name.clone());
        if let Some(lease_id) = &opts.lease_id {
            labels.insert(LABEL_LEASE_ID.to_string(), lease_id.clone());
            if let Some(identity) = &self.config.worker_identity {
                labels.insert(LABEL_WORKER_IDENTITY.to_string(), identity.clone());
            }
        }
        if let Some(tenant_id) = &opts.tenant_id {
            labels.insert(LABEL_TENANT_ID.to_string(), tenant_id.clone());
        }
        if let Some(digest) = &opts.digest_label {
            labels.insert(LABEL_DIGEST.to_string(), digest_label_value(digest));
        }

        let resource_map = container_resources(spec.cpu, spec.memory_mib);

        let mut env_vars = vec![
            EnvVar {
                name: "SANDBOX_PROVIDER".to_string(),
                value: Some(provider_str.clone()),
                ..Default::default()
            },
            EnvVar {
                name: "SANDBOX_OWNER".to_string(),
                value: Some(repo.owner.clone()),
                ..Default::default()
            },
            EnvVar {
                name: "SANDBOX_REPO".to_string(),
                value: Some(repo.name.clone()),
                ..Default::default()
            },
            EnvVar {
                name: "SANDBOX_RESOLVED_SHA".to_string(),
                value: Some(spec.resolved_sha.as_str().to_string()),
                ..Default::default()
            },
            EnvVar {
                name: "SANDBOX_WORKSPACE".to_string(),
                value: Some(WORKSPACE_PATH.to_string()),
                ..Default::default()
            },
            EnvVar {
                name: "SANDBOX_EVIDENCE_ROOT".to_string(),
                value: Some(EVIDENCE_PATH.to_string()),
                ..Default::default()
            },
            EnvVar {
                name: "SANDBOX_TOOLS".to_string(),
                value: Some(tools_csv),
                ..Default::default()
            },
        ];
        // The lease's own variables go first: Kubernetes keeps the last value
        // for a repeated name, so the provider's reserved variables win.
        env_vars.splice(
            0..0,
            spec.env.iter().map(|(name, value)| EnvVar {
                name: name.clone(),
                value: Some(value.clone()),
                ..Default::default()
            }),
        );
        if !spec.timeout.is_zero() {
            env_vars.push(EnvVar {
                name: "SANDBOX_TIMEOUT_SECS".to_string(),
                value: Some(spec.timeout.as_secs().to_string()),
                ..Default::default()
            });
        }
        if let Some(base) = spec.base_sha.as_ref() {
            env_vars.push(EnvVar {
                name: "SANDBOX_BASE_SHA".to_string(),
                value: Some(base.as_str().to_string()),
                ..Default::default()
            });
        }

        let clone_secret_name_opt: Option<String> = if let Some(creds) = &spec.clone {
            let secret_name = clone_secret_name(&job_name);
            run_phase(
                emitter,
                SandboxPhase::PrepareWorkspace,
                Some("git-clone".to_string()),
                || async {
                    let token_bytes = match &creds.auth {
                        CloneAuth::None(_) => vec![],
                        CloneAuth::BearerToken { token_ref } => {
                            resolve_secret_bytes(token_ref, self.jetstream.as_ref()).await?
                        }
                        CloneAuth::Basic { password_ref, .. } => {
                            resolve_secret_bytes(password_ref, self.jetstream.as_ref()).await?
                        }
                    };
                    let secret = build_clone_secret(
                        &secret_name,
                        &self.config.namespace,
                        &job_name,
                        &token_bytes,
                        self.config.resource_naming,
                    );
                    let created = self.ops.create_secret(&self.config.namespace, secret).await?;
                    self.record_created_resource(K8sResourceKind::Secret, &created.metadata)?;
                    Ok::<_, K8sProviderError>(())
                },
            )
            .await?;
            Some(secret_name)
        } else {
            None
        };

        let mut init_containers: Vec<Container> = Vec::new();
        if let (Some(creds), Some(secret_name)) = (&spec.clone, &clone_secret_name_opt) {
            init_containers.push(build_git_init_container(
                &creds.clone_url,
                spec.resolved_sha.as_str(),
                secret_name,
            ));
        }
        if opts.restore_secret_name.is_some() {
            init_containers.push(build_workspace_import_init_container(
                "restore-snapshot",
                RESTORE_VOLUME_NAME,
            ));
        }
        if opts.s3_volume_secret_name.is_some() {
            init_containers.push(build_workspace_import_init_container("s3-volume", "s3-volume-data"));
        }

        let image_pull_secrets: Option<Vec<k8s_openapi::api::core::v1::LocalObjectReference>> =
            if self.config.image_pull_secrets.is_empty() {
                None
            } else {
                Some(
                    self.config
                        .image_pull_secrets
                        .iter()
                        .map(|s| k8s_openapi::api::core::v1::LocalObjectReference { name: s.clone() })
                        .collect(),
                )
            };

        let runtime_class = self
            .runtime_class_override
            .clone()
            .unwrap_or_else(|| self.config.runtime_class.clone());

        if spec.privileged.is_granted() && !is_strong_isolation_runtime_class(&runtime_class) {
            return Err(K8sProviderError::PrivilegedNeedsStrongIsolation { runtime_class });
        }

        let default_ids = self.agent_ids_for_image(&spec.image.0).await;
        let mut agent_security_context = SecurityContext {
            run_as_non_root: Some(true),
            run_as_user: default_ids.0,
            run_as_group: default_ids.1,
            read_only_root_filesystem: Some(true),
            capabilities: Some(k8s_openapi::api::core::v1::Capabilities {
                drop: Some(vec!["ALL".to_string()]),
                add: None,
            }),
            ..Default::default()
        };
        if let Some((uid, gid)) = run_as_ids {
            agent_security_context.run_as_user = Some(uid);
            agent_security_context.run_as_group = gid;
        }
        if spec.privileged.is_granted() {
            agent_security_context.capabilities = Some(k8s_openapi::api::core::v1::Capabilities {
                drop: Some(vec!["ALL".to_string()]),
                add: Some(vec!["NET_ADMIN".to_string(), "SYS_ADMIN".to_string()]),
            });
            agent_security_context.read_only_root_filesystem = Some(false);
        }

        let mut agent_container = Container {
            name: "agent".to_string(),
            image: Some(spec.image.0.clone()),
            command: opts.keep_alive.then(keep_alive_command),
            image_pull_policy: Some("IfNotPresent".to_string()),
            resources: Some(ResourceRequirements {
                limits: Some(resource_map.clone()),
                requests: Some(resource_map),
                ..Default::default()
            }),
            env: Some(env_vars),
            security_context: Some(agent_security_context),
            volume_mounts: Some(vec![
                VolumeMount {
                    name: "workspace".to_string(),
                    mount_path: WORKSPACE_PATH.to_string(),
                    ..Default::default()
                },
                VolumeMount {
                    name: "evidence".to_string(),
                    mount_path: EVIDENCE_PATH.to_string(),
                    ..Default::default()
                },
                VolumeMount {
                    name: RUNTIME_TMP_VOLUME.into(),
                    mount_path: "/tmp".into(),
                    ..Default::default()
                },
            ]),
            ..Default::default()
        };

        if spec.privileged.is_granted() {
            agent_container
                .volume_mounts
                .get_or_insert_with(Vec::new)
                .push(VolumeMount {
                    name: "fuse-device".to_string(),
                    mount_path: "/dev/fuse".to_string(),
                    ..Default::default()
                });
        }

        let mut pod_selector_labels = BTreeMap::new();
        pod_selector_labels.insert(self.config.resource_naming.job_label().to_string(), job_name.clone());

        // `spec.network_policy` is the richer, newer source of truth. When absent,
        // fall back to translating the older `spec.egress_policy` enum into an
        // equivalent `NetworkPolicy` so both paths converge on
        // the same enforcement mechanism below.
        let effective_network_policy: NetworkPolicy =
            spec.network_policy
                .clone()
                .unwrap_or_else(|| match &spec.egress_policy {
                    EgressPolicy::DenyAll => NetworkPolicy::DenyAll,
                    EgressPolicy::AllowHosts(hosts) => NetworkPolicy::UserDefined {
                        rules: hosts
                            .iter()
                            .filter_map(|h| match h.parse() {
                                Ok(domain) => Some(DomainRule {
                                    domain,
                                    action: DomainRuleAction::PassThrough,
                                }),
                                Err(error) => {
                                    tracing::warn!(host = %h, %error, "dropping invalid egress host");
                                    None
                                }
                            })
                            .collect(),
                        allow_ranges: vec![],
                        deny_ranges: vec![],
                        denied_domains: vec![],
                        opaque_tunnels: Default::default(),
                        request_rules: vec![],
                    },
                });

        let network_policy_manifest = build_network_policy_manifest(
            &pod_selector_labels,
            &effective_network_policy,
            &self.config.namespace,
            &job_name,
        );

        let has_domain_rules = matches!(
            &effective_network_policy,
            NetworkPolicy::UserDefined { rules, .. } if !rules.is_empty()
        );

        let mut config_maps: Vec<ConfigMap> = Vec::new();
        let mut extra_volumes: Vec<Volume> = Vec::new();
        if spec.privileged.is_granted() {
            extra_volumes.push(Volume {
                name: "fuse-device".to_string(),
                host_path: Some(k8s_openapi::api::core::v1::HostPathVolumeSource {
                    path: "/dev/fuse".to_string(),
                    type_: Some("CharDevice".to_string()),
                }),
                ..Default::default()
            });
        }
        let mut sidecar_container = None;

        if has_domain_rules {
            let violation_events = match (
                self.violation_nats_url.as_ref(),
                opts.lease_id
                    .clone()
                    .map(LeaseId::new)
                    .or_else(|| spec.lease_id.clone()),
            ) {
                (Some(nats_url), Some(lease_id)) => EgressViolationEvents::try_new(nats_url, lease_id),
                _ => None,
            };
            if let Some(user) = spec.user.as_deref() {
                reject_proxy_uid_workload(user)?;
            }
            if let Some(security_context) = agent_container.security_context.as_mut() {
                security_context.allow_privilege_escalation = Some(false);
            }
            let sidecar = build_egress_sidecar(&effective_network_policy, &job_name, violation_events.as_ref())?;
            let ca_volume = sidecar.ca_volume;
            sidecar_container = Some(sidecar.sidecar_container);
            init_containers.push(sidecar.init_container);
            extra_volumes.push(sidecar.volume);
            config_maps.push(sidecar.config_map);

            // A `Transform`/`Forward` domain rule makes the sidecar terminate
            // TLS with its own per-Pod CA, so `agent` must trust it, or every
            // intercepted HTTPS request looks like a MITM to its TLS client.
            // `ca_volume` is only `Some` when such a rule is actually present
            // (see `build_egress_sidecar`), so a policy with only
            // `PassThrough` rules correctly skips this wiring instead of
            // pointing `agent`'s TLS clients at a CA file the sidecar never
            // populates.
            if let Some(ca_volume) = ca_volume {
                extra_volumes.push(ca_volume);
                let ca_file = format!("{EGRESS_PROXY_CA_MOUNT_PATH}/ca.pem");
                agent_container
                    .volume_mounts
                    .get_or_insert_with(Vec::new)
                    .push(VolumeMount {
                        name: EGRESS_PROXY_CA_VOLUME_NAME.to_string(),
                        mount_path: EGRESS_PROXY_CA_MOUNT_PATH.to_string(),
                        read_only: Some(true),
                        ..Default::default()
                    });
                agent_container
                    .env
                    .get_or_insert_with(Vec::new)
                    .extend(ca_trust_env(&ca_file).into_iter().map(|(name, value)| EnvVar {
                        name: name.to_string(),
                        value: Some(value),
                        ..Default::default()
                    }));
            }
        }

        let mut containers = vec![agent_container];
        if let Some(sidecar_container) = sidecar_container {
            containers.push(sidecar_container);
        }

        let pod_spec = PodSpec {
            runtime_class_name: runtime_class_name(&runtime_class),
            restart_policy: Some("Never".to_string()),
            termination_grace_period_seconds: Some(SANDBOX_TERMINATION_GRACE_SECONDS),
            service_account_name: Some(self.config.service_account.clone()),
            automount_service_account_token: Some(false),
            security_context: Some(hardened_pod_security_context()),
            init_containers: if init_containers.is_empty() {
                None
            } else {
                Some(init_containers)
            },
            containers,
            volumes: Some({
                let mut vols = vec![
                    Volume {
                        name: "workspace".to_string(),
                        empty_dir: Some(Default::default()),
                        ..Default::default()
                    },
                    Volume {
                        name: "evidence".to_string(),
                        empty_dir: Some(Default::default()),
                        ..Default::default()
                    },
                    runtime_tmp_volume(spec.memory_mib),
                ];
                if let Some(restore_secret) = &opts.restore_secret_name {
                    vols.push(Volume {
                        name: RESTORE_VOLUME_NAME.to_string(),
                        secret: Some(SecretVolumeSource {
                            secret_name: Some(restore_secret.clone()),
                            ..Default::default()
                        }),
                        ..Default::default()
                    });
                }
                if let Some(s3_secret) = &opts.s3_volume_secret_name {
                    vols.push(Volume {
                        name: "s3-volume-data".to_string(),
                        secret: Some(SecretVolumeSource {
                            secret_name: Some(s3_secret.clone()),
                            ..Default::default()
                        }),
                        ..Default::default()
                    });
                }
                vols.extend(extra_volumes);
                vols
            }),
            image_pull_secrets,
            ..Default::default()
        };

        let job = Job {
            metadata: ObjectMeta {
                name: Some(job_name.clone()),
                namespace: Some(self.config.namespace.clone()),
                labels: Some(labels),
                ..Default::default()
            },
            spec: Some(JobSpec {
                backoff_limit: Some(0),
                active_deadline_seconds: job_deadline_seconds(spec.timeout),
                template: PodTemplateSpec {
                    metadata: Some(ObjectMeta {
                        labels: Some(pod_selector_labels.clone()),
                        ..Default::default()
                    }),
                    spec: Some(pod_spec),
                },
                ..Default::default()
            }),
            ..Default::default()
        };

        let config_map_name_opt = if has_domain_rules {
            Some(egress_proxy_configmap_name(&job_name))
        } else {
            None
        };

        for config_map in config_maps {
            let namespace = self.config.namespace.clone();
            run_phase(
                emitter,
                SandboxPhase::PrepareWorkspace,
                Some("egress-proxy-configmap".to_string()),
                || async {
                    let created = self
                        .ops
                        .create_config_map(&namespace, config_map)
                        .await
                        .map_err(K8sProviderError::CreateConfigMap)?;
                    self.record_created_resource(K8sResourceKind::ConfigMap, &created.metadata)?;
                    Ok::<_, K8sProviderError>(())
                },
            )
            .await?;
        }

        let network_policy_name_opt = if let Some(netpol) = network_policy_manifest {
            let namespace = self.config.namespace.clone();
            run_phase(
                emitter,
                SandboxPhase::BootRuntime,
                Some("network-policy".to_string()),
                || async {
                    let created = self
                        .ops
                        .create_network_policy(&namespace, netpol)
                        .await
                        .map_err(K8sProviderError::CreateNetworkPolicy)?;
                    self.record_created_resource(K8sResourceKind::NetworkPolicy, &created.metadata)?;
                    Ok::<_, K8sProviderError>(())
                },
            )
            .await?;
            Some(network_policy_name(&job_name))
        } else {
            None
        };

        let runtime_detail = format!("{} (runtimeClass={})", spec.image.0, runtime_class);
        let created_job = run_phase(emitter, SandboxPhase::BootRuntime, Some(runtime_detail), || async {
            self.ops
                .create_job(&self.config.namespace, job)
                .await
                .map_err(K8sProviderError::CreateJob)
        })
        .await?;

        self.record_created_resource(K8sResourceKind::Job, &created_job.metadata)?;
        if self.receipts.is_some() {
            let owner =
                K8sResourceReceipt::from_metadata(K8sResourceKind::Job, &self.config.namespace, &created_job.metadata)?;
            self.attach_recorded_resources(&owner).await?;
        }

        // The Secrets are created before the Job they belong to, so ownership
        // is set afterwards. With it, deleting the Job by any path, including
        // a worker that restarted and no longer holds their names, also
        // deletes the Secrets.
        let owned_secrets: Vec<&str> = [
            clone_secret_name_opt.as_deref(),
            opts.restore_secret_name.as_deref(),
            opts.s3_volume_secret_name.as_deref(),
        ]
        .into_iter()
        .flatten()
        .collect();
        match created_job.metadata.uid.clone().filter(|_| self.receipts.is_none()) {
            Some(uid) => {
                let owner = OwnerReference {
                    api_version: "batch/v1".to_string(),
                    kind: "Job".to_string(),
                    name: job_name.clone(),
                    uid,
                    ..Default::default()
                };
                for secret in owned_secrets {
                    if let Err(error) = self
                        .ops
                        .set_secret_owner(&self.config.namespace, secret, owner.clone())
                        .await
                    {
                        tracing::warn!(job = %job_name, secret = %secret, error = %error, "failed to make the job own its secret");
                    }
                }
            }
            None if !owned_secrets.is_empty() && self.receipts.is_none() => {
                tracing::warn!(job = %job_name, "job created without a uid, its secrets have no owner");
            }
            None => {}
        }

        let (status_tx, status_rx) = watch::channel(SandboxStatus::Pending);
        let watcher_handle = self.ops.spawn_job_watcher(&self.config.namespace, &job_name, status_tx);

        Ok(Box::new(K8sMicroVmHandle {
            name: job_name,
            namespace: self.config.namespace.clone(),
            ops: Arc::clone(&self.ops),
            cancelled: Arc::new(AtomicBool::new(false)),
            clone_secret_name: clone_secret_name_opt,
            extra_secret_names: [opts.restore_secret_name.clone(), opts.s3_volume_secret_name.clone()]
                .into_iter()
                .flatten()
                .collect(),
            network_policy_name: network_policy_name_opt,
            config_map_name: config_map_name_opt,
            status_rx,
            _watcher_handle: Arc::new(watcher_handle),
        }))
    }

    /// Pushes an updated `policy` to an already-running sandbox, without ever
    /// touching its Job or Pod, so this is restart-free by construction: it
    /// only issues `PATCH` calls against the ConfigMap and/or NetworkPolicy
    /// objects the original launch created.
    ///
    /// For the ConfigMap path, kubelet re-syncs a mounted ConfigMap volume on
    /// its own periodic sync interval, so the sidecar's
    /// `sandbox-egress-proxyd` picks up the new file the same way it would
    /// pick up any other out-of-band edit, with no pod restart involved.
    ///
    /// For the NetworkPolicy path, `network_policy_name` being `None` after
    /// `build_network_policy_manifest` means the transition is to `AllowAll`:
    /// there is no such thing as an "empty but present" `NetworkPolicy` that
    /// means allow-all in this translation layer (see `k8s_network_policy`'s
    /// module docs), so the only correct way to reach allow-all is to delete
    /// the object outright rather than patch it to some placeholder spec. The
    /// reverse transition, from a lease that has no object, creates one.
    ///
    /// Returns the name of the NetworkPolicy now in force, `None` when there
    /// is none, so the caller can keep its record of the lease in step.
    pub async fn push_network_policy(
        &self,
        job_name: &str,
        configmap_name: Option<&str>,
        network_policy_name: Option<&str>,
        policy: &NetworkPolicy,
        sandbox_pod_labels: &BTreeMap<String, String>,
    ) -> Result<Option<String>, K8sProviderError> {
        if let Some(configmap_name) = configmap_name {
            let policy_json = serde_json::to_string(policy).map_err(K8sProviderError::SerializeNetworkPolicy)?;
            let mut data = BTreeMap::new();
            data.insert(EGRESS_PROXY_CONFIGMAP_KEY.to_string(), policy_json);
            self.ops
                .patch_config_map(&self.config.namespace, configmap_name, data)
                .await
                .map_err(K8sProviderError::PatchConfigMap)?;
        }

        let manifest = build_network_policy_manifest(sandbox_pod_labels, policy, &self.config.namespace, job_name);

        let Some(network_policy_name) = network_policy_name else {
            // A lease launched with `AllowAll` has no NetworkPolicy object, so
            // there is nothing to patch. Tightening it later means creating
            // one. A proxy-backed lease is enforced by its ConfigMap instead.
            let Some(netpol) = manifest else {
                return Ok(None);
            };
            if configmap_name.is_some() || policy_needs_egress_proxy(policy) {
                return Ok(None);
            }
            let name = netpol.metadata.name.clone();
            self.ops
                .create_network_policy(&self.config.namespace, netpol)
                .await
                .map_err(K8sProviderError::CreateNetworkPolicy)?;
            return Ok(name);
        };

        match manifest {
            Some(netpol) => {
                let spec = netpol.spec.ok_or(K8sProviderError::NetworkPolicyManifestMissingSpec)?;
                self.ops
                    .patch_network_policy(&self.config.namespace, network_policy_name, spec)
                    .await
                    .map_err(K8sProviderError::PatchNetworkPolicy)?;
                Ok(Some(network_policy_name.to_string()))
            }
            None => {
                self.ops
                    .delete_network_policy(&self.config.namespace, network_policy_name)
                    .await
                    .map_err(K8sProviderError::DeleteNetworkPolicy)?;
                Ok(None)
            }
        }
    }
}
