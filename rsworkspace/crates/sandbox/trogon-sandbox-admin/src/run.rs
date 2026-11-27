//! Maps a parsed command line onto the library commands.

use std::time::Duration;

use anyhow::{Context, anyhow};
use async_nats::Client;
use async_nats::jetstream;
use trogon_sandbox_admin::{
    PrebuildRequest, api_key, capability, constants, credential, lease, prewarm, quota, snapshot, substrate, tag,
    tenant_policy, worker,
};
use trogon_sandbox_auth::NatsKvApiKeyStore;
use trogon_sandbox_broker::active_lease::NatsKvActiveLeaseStore;
use trogon_sandbox_broker::freeze_admin::FreezeResolution;
use trogon_sandbox_broker::host_mount_policy::NatsKvHostMountAllowList;
use trogon_sandbox_broker::idempotency::NatsKvIdempotencyStore;
use trogon_sandbox_broker::image_policy::NatsKvImageAllowList;
use trogon_sandbox_broker::limits::{NatsKvTenantLimits, PerTagQuota};
use trogon_sandbox_broker::native_policy::NatsKvNativePassthroughStore;
use trogon_sandbox_broker::quota::NatsKvQuotaStore;
use trogon_sandbox_broker::worker_fence::WorkerPlacementState;
use trogon_sandbox_contract::SnapshotTag;
use trogon_sandbox_credential_broker::InProcessKey;

use crate::cli::{
    ApiKeyCommand, CapabilityCommand, Cli, Command, CredentialCommand, HostMountCommand, ImagePolicyCommand,
    LeaseCommand, NativePassthroughCommand, PrewarmCommand, QuotaCommand, SnapshotCommand, SubstrateCommand,
    TagCommand, TenantLimitCommand, WorkerCommand,
};
use trogon_sandbox_admin::Console;

pub async fn run(cli: Cli, nats: Client, console: &mut Console<'_>) -> anyhow::Result<()> {
    let js = jetstream::new(nats.clone());
    let Cli {
        api_key_bucket,
        kms_key,
        command,
        nats_url: _,
    } = cli;

    match command {
        Command::ApiKey(command) => {
            let store = NatsKvApiKeyStore::new_with_bucket(js, &api_key_bucket)
                .await
                .context("open api-key store")?;
            match command {
                ApiKeyCommand::Mint(args) => {
                    api_key::mint(&store, args.tenant, args.service_account, &args.name, console).await?;
                }
                ApiKeyCommand::List(args) => api_key::list(&store, args.tenant.as_ref(), console).await?,
                ApiKeyCommand::Revoke(args) => api_key::revoke(&store, &args.token, console).await?,
            }
        }
        Command::ImagePolicy(command) => {
            let store = NatsKvImageAllowList::new(js, constants::IMAGE_ALLOWLIST_BUCKET)
                .await
                .context("open image-allowlist store")?;
            match command {
                ImagePolicyCommand::Set(args) => {
                    tenant_policy::image_policy_set(&store, &args.tenant, args.patterns, args.replace, console).await?;
                }
                ImagePolicyCommand::Show(args) => {
                    tenant_policy::image_policy_show(&store, &args.tenant, console).await?
                }
                ImagePolicyCommand::Clear(args) => {
                    tenant_policy::image_policy_clear(&store, &args.tenant, console).await?;
                }
            }
        }
        Command::HostMount(command) => {
            let store = NatsKvHostMountAllowList::new(js, constants::HOST_MOUNT_ALLOWLIST_BUCKET)
                .await
                .context("open host-mount-allowlist store")?;
            match command {
                HostMountCommand::Set(args) => {
                    tenant_policy::host_mount_set(&store, &args.tenant, args.prefixes, args.replace, console).await?;
                }
                HostMountCommand::Show(args) => tenant_policy::host_mount_show(&store, &args.tenant, console).await?,
                HostMountCommand::Clear(args) => {
                    tenant_policy::host_mount_clear(&store, &args.tenant, console).await?;
                }
            }
        }
        Command::NativePassthrough(command) => {
            let store = NatsKvNativePassthroughStore::new(js, constants::NATIVE_PASSTHROUGH_BUCKET)
                .await
                .context("open native-passthrough store")?;
            match command {
                NativePassthroughCommand::Set(args) => {
                    tenant_policy::native_passthrough_set(&store, &args.tenant, !args.deny, console).await?;
                }
                NativePassthroughCommand::Show(args) => {
                    tenant_policy::native_passthrough_show(&store, &args.tenant, console).await?;
                }
                NativePassthroughCommand::Clear(args) => {
                    tenant_policy::native_passthrough_clear(&store, &args.tenant, console).await?;
                }
            }
        }
        Command::TenantLimit(command) => {
            let store = NatsKvTenantLimits::new(js, constants::TENANT_LIMITS_BUCKET)
                .await
                .context("open tenant-limits store")?;
            match command {
                TenantLimitCommand::Set(args) => {
                    let per_tag_quota = args
                        .per_tag_key
                        .zip(args.per_tag_max)
                        .map(|(tag_key, max_active_leases)| PerTagQuota {
                            tag_key,
                            max_active_leases,
                        });
                    tenant_policy::tenant_limit_set(
                        &store,
                        &args.tenant,
                        args.max_cpu_seconds,
                        args.max_memory_mib,
                        per_tag_quota,
                        console,
                    )
                    .await?;
                }
                TenantLimitCommand::Show(args) => {
                    tenant_policy::tenant_limit_show(&store, &args.tenant, console).await?
                }
                TenantLimitCommand::Clear(args) => {
                    tenant_policy::tenant_limit_clear(&store, &args.tenant, console).await?;
                }
            }
        }
        Command::Quota(QuotaCommand::Show(args)) => {
            let store = NatsKvQuotaStore::new(js, constants::QUOTA_BUCKET)
                .await
                .context("open quota store")?;
            quota::show(&store, &args.tenant, args.month, console).await?;
        }
        Command::Lease(command) => {
            let (resolution, args) = match command {
                LeaseCommand::ClearFreeze(args) => (FreezeResolution::Clear, args),
                LeaseCommand::RetryFreeze(args) => (FreezeResolution::Retry, args),
            };
            let active = NatsKvActiveLeaseStore::new(js.clone(), &args.active_lease_bucket)
                .await
                .context("open active-lease store")?;
            let idempotency = NatsKvIdempotencyStore::new(js, &args.idempotency_bucket)
                .await
                .context("open idempotency store")?;
            lease::freeze(&active, &idempotency, &args.lease_id, resolution, args.force, console).await?;
        }
        Command::Capability(CapabilityCommand::Show(args)) => {
            capability::show(&nats, args.provider, console).await?;
        }
        Command::Capability(CapabilityCommand::Diff(args)) => {
            capability::diff(&nats, args.provider_a, args.provider_b, console).await?;
        }
        Command::Credential(command) => run_credential(command, js, nats, kms_key, console).await?,
        Command::Tag(command) => run_tag(command, js, console).await?,
        Command::Worker(command) => run_worker(command, js, console).await?,
        Command::Prewarm(command) => match command {
            PrewarmCommand::Set(args) => {
                prewarm::set(js, &args.tenant, &args.digest, args.size, args.providers, console).await?;
            }
            PrewarmCommand::Show(args) => prewarm::show(js, &args.tenant, console).await?,
            PrewarmCommand::Clear(args) => prewarm::clear(js, &args.tenant, &args.digest, console).await?,
        },
        Command::Snapshot(command) => match command {
            SnapshotCommand::Delete(args) => {
                snapshot::delete(
                    snapshot::SnapshotDeleteRequest {
                        registry: args.registry,
                        insecure_registry: args.insecure_registry,
                        tenant: args.tenant,
                        digest: args.digest,
                        artifact_type: args.artifact_type,
                    },
                    console,
                )
                .await?;
            }
            SnapshotCommand::Status(args) => snapshot::status(js, &args.lease_id, console).await?,
        },
        Command::Substrate(SubstrateCommand::Prebuild(args)) => {
            substrate::prebuild(
                js,
                PrebuildRequest {
                    ate_api_endpoint: args.ate_api_endpoint,
                    tls_ca_file: args.tls_ca_file,
                    tls_server_name: args.tls_server_name,
                    bearer_token_file: args.bearer_token_file,
                    tenant: args.tenant,
                    image: args.image,
                    sandbox_class: args.sandbox_class,
                    agent_image: args.agent_image,
                    agent_port: args.agent_port,
                    size_class_label: args.size_class,
                    snapshots_location: args.snapshots_location,
                    actor_cpu_millicores: args.actor_cpu_millicores,
                    memory_mib: args.memory_mib,
                    timeout: Duration::from_secs(args.timeout_seconds),
                },
                console,
            )
            .await?;
        }
    }
    Ok(())
}

async fn run_tag(command: TagCommand, js: jetstream::Context, console: &mut Console<'_>) -> anyhow::Result<()> {
    match command {
        TagCommand::Set(args) => {
            let tag = SnapshotTag {
                tenant: args.tenant,
                name: args.name,
            };
            tag::set(
                js,
                tag,
                args.digest,
                args.artifact_type,
                args.expected_revision,
                console,
            )
            .await?;
        }
        TagCommand::Get(args) => {
            let tag = SnapshotTag {
                tenant: args.tenant,
                name: args.name,
            };
            tag::get(js, &tag, console).await?;
        }
        TagCommand::List(args) => tag::list(js, &args.tenant, console).await?,
        TagCommand::History(args) => tag::history(js, &args.tenant, &args.name, console).await?,
        TagCommand::Clear(args) => {
            let tag = SnapshotTag {
                tenant: args.tenant,
                name: args.name,
            };
            tag::clear(js, &tag, console).await?;
        }
    }
    Ok(())
}

async fn run_worker(command: WorkerCommand, js: jetstream::Context, console: &mut Console<'_>) -> anyhow::Result<()> {
    let (args, state) = match command {
        WorkerCommand::Cordon(args) => (args, WorkerPlacementState::Cordoned),
        WorkerCommand::Drain(args) => (args, WorkerPlacementState::Draining),
        WorkerCommand::Uncordon(args) => (args, WorkerPlacementState::Active),
        WorkerCommand::Status(target) => {
            worker::status(
                js,
                &target.active_lease_bucket,
                target.provider,
                &target.worker,
                console,
            )
            .await?;
            return Ok(());
        }
        WorkerCommand::List => {
            worker::list(js, console).await?;
            return Ok(());
        }
    };
    worker::set_state(
        js,
        &args.target.active_lease_bucket,
        args.target.provider,
        &args.target.worker,
        state,
        args.reason,
        console,
    )
    .await?;
    Ok(())
}

fn require_kms_key(key: Option<InProcessKey>) -> anyhow::Result<InProcessKey> {
    key.ok_or_else(|| anyhow!("--kms-key (or SANDBOX_KMS_KEY) is required for this command"))
}

async fn run_credential(
    command: CredentialCommand,
    js: jetstream::Context,
    nats: Client,
    kms_key: Option<InProcessKey>,
    console: &mut Console<'_>,
) -> anyhow::Result<()> {
    match command {
        CredentialCommand::Set(args) => {
            let material = std::io::read_to_string(std::io::stdin()).context("read credential material from stdin")?;
            let key = require_kms_key(kms_key)?;
            credential::set(
                js,
                key,
                args.provider,
                &args.tenant,
                &args.credential_id,
                &material,
                console,
            )
            .await?;
        }
        CredentialCommand::List(args) => {
            credential::list(js, require_kms_key(kms_key)?, args.provider, console).await?;
        }
        CredentialCommand::Rotate(args) => {
            let key = require_kms_key(kms_key)?;
            credential::rotate(js, key, args.provider, &args.tenant, &args.credential_id, console).await?;
        }
        CredentialCommand::Migrate(args) => {
            let request = credential::MigrateRequest {
                tenant: args.tenant,
                provider: args.provider.0,
                dry_run: args.dry_run,
                only: args.credential_ids,
            };
            credential::migrate(js, require_kms_key(kms_key)?, request, console).await?;
        }
        CredentialCommand::RevokeToken(args) => {
            credential::revoke_token(nats, args.provider, args.tenant, args.token_id, console).await?;
        }
    }
    Ok(())
}
