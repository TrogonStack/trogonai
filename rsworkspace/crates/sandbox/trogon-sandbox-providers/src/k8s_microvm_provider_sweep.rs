use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, PoisonError};

use trogon_sandbox_contract::SandboxPhase;
use trogon_sandbox_protocol::{LifecycleEmitter, run_phase};

use crate::k8s_ops::constants::JOB_POD_POLL_INTERVAL;
use crate::k8s_ops::{
    JobPodReadiness, clone_secret_name, job_outlived_deadline_by, job_pod_readiness, warm_pod_outlived_deadline_by,
};
use crate::{
    ApiServerClock, FOREIGN_LEASE_GRACE_SECONDS, K8sMicroVmProvider, K8sProviderError, K8sResourceNaming,
    LABEL_LEASE_DEADLINE, LABEL_LEASE_ID, LABEL_TENANT_ID, LABEL_WARM_POOL, LABEL_WORKER_IDENTITY, LeaseJobSweep,
    RecoveredLeaseJob, RecoveredWarmLease, WARM_POOL_CLAIMED, job_is_finished, job_outlived_deadline,
    lease_request_secret_name, network_policy_name, restore_secret_name, s3_volume_secret_name,
    warm_pod_restore_secret_name,
};

impl K8sMicroVmProvider {
    /// Deletes the Job (and clone Secret and NetworkPolicy when present). Emits [`SandboxPhase::Reclaim`] when a lifecycle emitter is set.
    pub async fn tear_down(
        &self,
        job_name: &str,
        clone_secret_name: Option<&str>,
        extra_secret_names: &[&str],
        network_policy_name: Option<&str>,
        lifecycle: Option<&dyn LifecycleEmitter>,
    ) -> Result<(), K8sProviderError> {
        let namespace = self.config.namespace.clone();
        let ops = Arc::clone(&self.ops);
        let job = job_name.to_string();
        let secret = clone_secret_name.map(str::to_string);
        let extras: Vec<String> = extra_secret_names.iter().map(|s| s.to_string()).collect();
        let netpol = network_policy_name.map(str::to_string);

        run_phase(
            lifecycle,
            SandboxPhase::Reclaim,
            Some(job_name.to_string()),
            || async move {
                ops.delete_job(&namespace, &job).await?;
                if let Some(secret_name) = secret.as_deref() {
                    ops.delete_secret(&namespace, secret_name).await?;
                }
                for secret_name in &extras {
                    ops.delete_secret(&namespace, secret_name).await?;
                }
                if let Some(name) = netpol.as_deref() {
                    ops.delete_network_policy(&namespace, name).await?;
                }
                Ok(())
            },
        )
        .await
    }

    /// Finds the cold lease Jobs in the namespace, tears down those whose lease
    /// can no longer run, and reports the rest with the pod each one execs in.
    ///
    /// A Job is torn down once the Job controller finished it or it outlived
    /// its `activeDeadlineSeconds` on the apiserver clock. With no clock in the
    /// listing only finished Jobs are torn down: a deferred cleanup costs an
    /// idle Job, a wrong one kills a live sandbox. A Job already being deleted
    /// is neither recovered nor torn down again.
    pub async fn sweep_lease_jobs(&self) -> Result<LeaseJobSweep, K8sProviderError> {
        let listing = self.ops.list_jobs(&self.config.namespace, LABEL_LEASE_ID).await?;
        for job in &listing.jobs {
            if let Some(labels) = &job.metadata.labels
                && labels.contains_key(LABEL_LEASE_ID)
            {
                self.ensure_resource_naming(labels)?;
            }
        }
        let mut sweep = LeaseJobSweep::default();
        let mut live_owners = None;
        for job in &listing.jobs {
            if job.metadata.deletion_timestamp.is_some() {
                continue;
            }
            let Some(job_name) = job.metadata.name.clone() else {
                continue;
            };
            let labels = job.metadata.labels.clone().unwrap_or_default();
            let Some(lease_id) = labels.get(LABEL_LEASE_ID).cloned() else {
                continue;
            };
            // Another replica's Job is left to it while it can still sweep it.
            // Only a deadline passed by [`FOREIGN_LEASE_GRACE_SECONDS`] marks
            // its owner as gone, and expiry is decided on the apiserver clock,
            // never on whether the owner is alive. Finished Jobs stay with the
            // owner, which still needs to see them to forget the lease.
            if !self.config.owns_lease_resource(&labels) {
                let mut abandoned = listing
                    .clock
                    .is_some_and(|now| job_outlived_deadline_by(job, now, FOREIGN_LEASE_GRACE_SECONDS));
                // A Job with no deadline never expires, so its owner being
                // gone is the only sign it was abandoned.
                if !abandoned
                    && let Some(now) = listing.clock
                    && job
                        .spec
                        .as_ref()
                        .and_then(|spec| spec.active_deadline_seconds)
                        .is_none()
                {
                    abandoned = self
                        .is_orphaned(&labels, job.metadata.creation_timestamp.as_ref(), now, &mut live_owners)
                        .await?;
                }
                if abandoned {
                    match self.tear_down_lease_job(&job_name).await {
                        Ok(()) => sweep.swept.push(job_name),
                        Err(error) => {
                            tracing::warn!(job = %job_name, error = %error, "failed to tear down an abandoned lease job");
                        }
                    }
                }
                continue;
            }
            let expired = job_is_finished(job) || listing.clock.is_some_and(|now| job_outlived_deadline(job, now));
            if expired {
                match self.tear_down_lease_job(&job_name).await {
                    Ok(()) => sweep.swept.push(job_name),
                    Err(error) => {
                        tracing::warn!(job = %job_name, error = %error, "failed to tear down an expired lease job");
                    }
                }
                continue;
            }
            match self.running_job_pod(&job_name).await? {
                Some(pod_name) => sweep.live.push(RecoveredLeaseJob {
                    lease_id,
                    tenant_id: labels.get(LABEL_TENANT_ID).cloned(),
                    job_name,
                    pod_name,
                }),
                None => {
                    tracing::info!(job = %job_name, "lease job has no running pod yet, leaving it for the next sweep");
                }
            }
        }
        Ok(sweep)
    }

    /// Whether a lease resource this replica does not own, which has no
    /// deadline, was abandoned by an owner that no longer exists.
    ///
    /// Two cases qualify, each only once the resource is
    /// `orphan_reclaim_grace_seconds` old on the apiserver clock:
    ///
    /// - It names an owner identity and no pod of that name has been in the
    ///   namespace for that long, as when a StatefulSet scaled in past that
    ///   ordinal. The absence must be continuous across sweeps: a rolling
    ///   restart removes the owner's pod for a few seconds, and the pod that
    ///   comes back under the same name re-adopts its leases, so one sweep
    ///   that misses it proves nothing. A pod in any phase, terminating or not
    ///   yet Ready, counts as the owner being there. This needs identities to
    ///   be pod names, which is proven by this replica's own identity being a
    ///   pod it can see; otherwise nothing is reclaimed.
    /// - It names no owner and this replica does not adopt unlabeled leases
    ///   (`recover_unlabeled_leases` off), so no replica ever will. With the
    ///   adoption on, such a resource may be live and in some replica's table,
    ///   and nothing tells it apart from a leaked one, so it is left alone.
    ///
    /// `live_owners` caches the pod names for the length of one sweep.
    pub(crate) async fn is_orphaned(
        &self,
        labels: &BTreeMap<String, String>,
        created: Option<&k8s_openapi::apimachinery::pkg::apis::meta::v1::Time>,
        now: ApiServerClock,
        live_owners: &mut Option<Option<BTreeSet<String>>>,
    ) -> Result<bool, K8sProviderError> {
        let grace = self.config.orphan_reclaim_grace_seconds;
        let Some(me) = self.config.worker_identity.as_deref() else {
            return Ok(false);
        };
        let Some(created) = created else {
            return Ok(false);
        };
        if grace <= 0 {
            return Ok(false);
        }
        let old_enough = now.as_second() - created.0.as_second() >= grace;
        let Some(owner) = labels.get(LABEL_WORKER_IDENTITY) else {
            return Ok(old_enough && !self.config.recover_unlabeled_leases);
        };
        if owner == me {
            return Ok(false);
        }
        if live_owners.is_none() {
            let pods = self.ops.list_pods(&self.config.namespace, "").await?.pods;
            let names: BTreeSet<String> = pods.into_iter().filter_map(|pod| pod.metadata.name).collect();
            let mut absent = self.owner_absence.lock().unwrap_or_else(PoisonError::into_inner);
            absent.retain(|identity, _| !names.contains(identity));
            drop(absent);
            *live_owners = Some(names.contains(me).then_some(names));
        }
        let Some(Some(names)) = live_owners.as_ref() else {
            return Ok(false);
        };
        if names.contains(owner) {
            return Ok(false);
        }
        let mut absent = self.owner_absence.lock().unwrap_or_else(PoisonError::into_inner);
        let (first, last) = absent
            .entry(owner.clone())
            .or_insert((now.as_second(), now.as_second()));
        // Sweeps run every minute, so a long gap in sightings means the
        // absence was not watched and the count starts over.
        if now.as_second() - *last > FOREIGN_LEASE_GRACE_SECONDS {
            *first = now.as_second();
        }
        *last = now.as_second();
        Ok(old_enough && now.as_second() - *first >= grace)
    }

    /// Tears down cold lease Job `job_name` and everything a launch may have
    /// created for it, each by its derived name. Missing objects are fine.
    pub async fn tear_down_lease_job(&self, job_name: &str) -> Result<(), K8sProviderError> {
        let clone_secret = clone_secret_name(job_name);
        let others = [
            restore_secret_name(job_name),
            s3_volume_secret_name(job_name),
            lease_request_secret_name(job_name),
        ];
        let others: Vec<&str> = others.iter().map(String::as_str).collect();
        let netpol = network_policy_name(job_name);
        self.tear_down(job_name, Some(&clone_secret), &others, Some(&netpol), None)
            .await
    }

    /// The warm pods claimed for a lease, with the restore Secret each mounts.
    pub async fn claimed_warm_pods(&self) -> Result<Vec<RecoveredWarmLease>, K8sProviderError> {
        let selector = format!("{LABEL_WARM_POOL}={WARM_POOL_CLAIMED}");
        let listing = self.ops.list_pods(&self.config.namespace, &selector).await?;
        for pod in &listing.pods {
            if let Some(labels) = &pod.metadata.labels {
                self.ensure_resource_naming(labels)?;
            }
        }
        Ok(listing
            .pods
            .iter()
            .filter(|pod| pod.metadata.deletion_timestamp.is_none())
            .filter_map(|pod| {
                let labels = pod.metadata.labels.as_ref()?;
                if labels.get(LABEL_WARM_POOL).map(String::as_str) != Some(WARM_POOL_CLAIMED)
                    || !self.config.resource_naming.owns_warm_labels(labels)
                {
                    return None;
                }
                if !self.config.owns_lease_resource(labels) {
                    return None;
                }
                Some(RecoveredWarmLease {
                    lease_id: labels.get(LABEL_LEASE_ID)?.clone(),
                    pod_name: pod.metadata.name.clone()?,
                    restore_secret_name: warm_pod_restore_secret_name(pod),
                })
            })
            .collect())
    }

    /// Deletes the claimed warm pods this worker owns whose lease outlived the
    /// deadline its claim stamped, with the restore Secret and the
    /// `NetworkPolicy` each one may hold, and returns the names of the pods
    /// deleted.
    ///
    /// This is the warm-pod counterpart of the deadline half of
    /// [`Self::sweep_lease_jobs`]: the same apiserver clock decides, and with
    /// no clock in the listing nothing is swept. Another replica's pod is left
    /// to it until it is [`FOREIGN_LEASE_GRACE_SECONDS`] past its deadline, and
    /// a pod already being deleted is not deleted again.
    pub async fn sweep_claimed_warm_pods(&self) -> Result<Vec<String>, K8sProviderError> {
        let selector = format!("{LABEL_WARM_POOL}={WARM_POOL_CLAIMED}");
        let listing = self.ops.list_pods(&self.config.namespace, &selector).await?;
        for pod in &listing.pods {
            if let Some(labels) = &pod.metadata.labels {
                self.ensure_resource_naming(labels)?;
            }
        }
        let Some(now) = listing.clock else {
            tracing::debug!("no apiserver clock in the claimed warm pod listing, skipping the deadline sweep");
            return Ok(Vec::new());
        };
        let mut swept = Vec::new();
        let mut live_owners = None;
        for pod in &listing.pods {
            if pod.metadata.deletion_timestamp.is_some() {
                continue;
            }
            let Some(name) = pod.metadata.name.clone() else {
                continue;
            };
            let Some(labels) = pod.metadata.labels.as_ref() else {
                continue;
            };
            if labels.get(LABEL_WARM_POOL).map(String::as_str) != Some(WARM_POOL_CLAIMED)
                || !self.config.resource_naming.owns_warm_labels(labels)
            {
                continue;
            }
            let owned = self.config.owns_lease_resource(labels);
            let grace = if owned { 0 } else { FOREIGN_LEASE_GRACE_SECONDS };
            if !warm_pod_outlived_deadline_by(pod, now, grace) {
                let no_deadline = !labels.contains_key(LABEL_LEASE_DEADLINE);
                if owned
                    || !no_deadline
                    || !self
                        .is_orphaned(labels, pod.metadata.creation_timestamp.as_ref(), now, &mut live_owners)
                        .await?
                {
                    continue;
                }
            }
            let secret = warm_pod_restore_secret_name(pod);
            let request_secret = lease_request_secret_name(&name);
            let mut secrets: Vec<&str> = secret.as_deref().into_iter().collect();
            secrets.push(&request_secret);
            let netpol = network_policy_name(&name);
            let result = match self.delete_warm_pod(&name, &secrets).await {
                Ok(()) => self.delete_network_policy(&netpol).await,
                Err(error) => Err(error),
            };
            match result {
                Ok(()) => swept.push(name),
                Err(error) => {
                    tracing::warn!(pod = %name, error = %error, "failed to delete an expired claimed warm pod");
                }
            }
        }
        Ok(swept)
    }

    /// The pod of Job `job_name` whose `agent` container runs, if one does.
    async fn running_job_pod(&self, job_name: &str) -> Result<Option<String>, K8sProviderError> {
        let listing = self
            .ops
            .list_pods(&self.config.namespace, &format!("job-name={job_name}"))
            .await?;
        Ok(listing.pods.iter().find_map(|pod| {
            (job_pod_readiness(pod) == JobPodReadiness::Ready)
                .then(|| pod.metadata.name.clone())
                .flatten()
        }))
    }

    /// Waits until the pod of Job `job_name` runs its `agent` container and
    /// returns the pod's name, which is what exec needs. Fails early when the
    /// pod cannot start (bad image, a security context the image violates),
    /// rather than handing out a sandbox nothing can run in.
    pub async fn wait_for_job_pod(
        &self,
        job_name: &str,
        timeout: std::time::Duration,
    ) -> Result<String, K8sProviderError> {
        let deadline = tokio::time::Instant::now() + timeout;
        let selector = format!("job-name={job_name}");
        loop {
            let listing = self.ops.list_pods(&self.config.namespace, &selector).await?;
            for pod in &listing.pods {
                let Some(name) = pod.metadata.name.clone() else {
                    continue;
                };
                match job_pod_readiness(pod) {
                    JobPodReadiness::Ready => return Ok(name),
                    JobPodReadiness::Failed(reason) => {
                        return Err(K8sProviderError::PodFailedToStart { pod: name, reason });
                    }
                    JobPodReadiness::Starting => {}
                }
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(K8sProviderError::PodStartTimeout {
                    job: job_name.to_string(),
                    seconds: timeout.as_secs(),
                });
            }
            tokio::time::sleep(JOB_POD_POLL_INTERVAL).await;
        }
    }

    /// Waits until Job `job_name` and its pods are gone from the cluster.
    /// Deleting a Job returns at once and its pods follow, and a Job launched
    /// under the same name meanwhile would find the old pod first.
    pub async fn wait_for_job_gone(
        &self,
        job_name: &str,
        timeout: std::time::Duration,
    ) -> Result<(), K8sProviderError> {
        let deadline = tokio::time::Instant::now() + timeout;
        let pod_selector = format!("job-name={job_name}");
        loop {
            let jobs = self.ops.list_jobs(&self.config.namespace, LABEL_LEASE_ID).await?;
            let job_present = jobs
                .jobs
                .iter()
                .any(|job| job.metadata.name.as_deref() == Some(job_name));
            let pods_present = !self
                .ops
                .list_pods(&self.config.namespace, &pod_selector)
                .await?
                .pods
                .is_empty();
            if !job_present && !pods_present {
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(K8sProviderError::JobDeletionTimeout {
                    job: job_name.to_string(),
                    seconds: timeout.as_secs(),
                });
            }
            tokio::time::sleep(JOB_POD_POLL_INTERVAL).await;
        }
    }

    pub fn resource_naming(&self) -> K8sResourceNaming {
        self.config.resource_naming
    }
}
