use std::collections::BTreeMap;

use async_trait::async_trait;
use futures::{SinkExt, StreamExt};
use k8s_openapi::api::batch::v1::Job;
use k8s_openapi::api::core::v1::{ConfigMap, Pod, Secret};
use k8s_openapi::api::networking::v1::{NetworkPolicy, NetworkPolicySpec};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{ObjectMeta, OwnerReference};
use kube::api::{AttachParams, DeleteParams, ListParams, Patch, PatchParams, PostParams, TerminalSize};
use kube::runtime::watcher::{self, Config as WatcherConfig};
use kube::{Api, Client};
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{mpsc, watch};
use trogon_sandbox_contract::{OutputRetention, OutputTruncation};

use crate::k8s_ops::constants::TTY_RESIZE_QUEUE;
use crate::k8s_ops::patch_bodies::{
    ConfigMapDataPatch, FencedLabelsPatch, NetworkPolicySpecPatch, OwnerReferencesPatch,
};
use crate::k8s_ops::{
    ApiServerClock, FencedPatchOutcome, InteractivePodExec, JobListing, JobPhase, K8sOps, PodExecOutput, PodListing,
    TtyPodExec, exit_code_from_status, job_delete_params, job_phase_from_k8s, parse_http_date, read_capped, read_tail,
};
use crate::{K8sOpsError, K8sResourceKind, K8sResourceReceipt, SandboxStatus};

/// The [`K8sOps`] that talks to a real apiserver through a `kube` client.
pub(crate) struct RealK8sOps {
    pub(crate) client: Client,
}

#[derive(Serialize)]
struct LabelSelectorQuery<'a> {
    #[serde(rename = "labelSelector")]
    label_selector: &'a str,
}

fn immediate_delete_params() -> DeleteParams {
    DeleteParams {
        grace_period_seconds: Some(0),
        ..Default::default()
    }
}

/// Maps the apiserver's `404 Not Found` to success, since a delete's goal is
/// that the object is gone.
fn deleted_or_missing<T>(result: Result<T, kube::Error>) -> Result<(), K8sOpsError> {
    match result {
        Ok(_) => Ok(()),
        Err(kube::Error::Api(status)) if status.code == 404 => Ok(()),
        Err(error) => Err(error.into()),
    }
}

impl RealK8sOps {
    pub(crate) fn new(client: Client) -> Self {
        Self { client }
    }

    fn pods(&self, namespace: &str) -> Api<Pod> {
        Api::namespaced(self.client.clone(), namespace)
    }

    /// Issued as a raw request rather than through `Api::list` because the
    /// typed client discards response headers, and the `Date` header is the
    /// only clock here that is not a worker host's.
    async fn list_with_clock<T>(
        &self,
        path: &str,
        label_selector: &str,
    ) -> Result<(Vec<T>, Option<ApiServerClock>), K8sOpsError>
    where
        T: k8s_openapi::ListableResource + DeserializeOwned + Clone,
    {
        let query =
            serde_urlencoded::to_string(LabelSelectorQuery { label_selector }).map_err(K8sOpsError::LabelSelector)?;
        let request = http::Request::get(format!("{path}?{query}"))
            .body(kube::client::Body::empty())
            .map_err(K8sOpsError::ListRequest)?;

        let response = self
            .client
            .send(request)
            .await
            .map_err(|source| K8sOpsError::ListSend {
                path: path.to_string(),
                source: Box::new(source),
            })?;

        let clock = response
            .headers()
            .get(http::header::DATE)
            .and_then(|value| value.to_str().ok())
            .and_then(parse_http_date);

        let status = response.status();
        let body = http_body_util::BodyExt::collect(response.into_body())
            .await
            .map_err(|error| K8sOpsError::ListBody(error.into()))?
            .to_bytes();
        if !status.is_success() {
            return Err(K8sOpsError::ListStatus {
                path: path.to_string(),
                status,
                body: String::from_utf8_lossy(&body).into_owned(),
            });
        }

        let list: k8s_openapi::List<T> = serde_json::from_slice(&body).map_err(K8sOpsError::ListDecode)?;
        Ok((list.items, clock))
    }

    async fn start_exec(
        &self,
        namespace: &str,
        pod_name: &str,
        command: &[String],
        params: &AttachParams,
    ) -> Result<kube::api::AttachedProcess, K8sOpsError> {
        let cmd: Vec<&str> = command.iter().map(String::as_str).collect();
        Ok(self.pods(namespace).exec(pod_name, cmd, params).await?)
    }
}

pub(crate) fn exec_io(action: &'static str, stream: &'static str) -> impl FnOnce(std::io::Error) -> K8sOpsError {
    move |source| K8sOpsError::ExecIo { action, stream, source }
}

/// The stream the exec asked for. `kube` returns `None` only for a stream
/// that was not requested or was already taken.
pub(crate) fn require_stream<T>(stream: Option<T>, name: &'static str) -> Result<T, K8sOpsError> {
    stream.ok_or(K8sOpsError::ExecStreamMissing { stream: name })
}

fn exec_join_error(error: impl std::error::Error + Send + Sync + 'static) -> K8sOpsError {
    K8sOpsError::ExecJoin(Box::new(error))
}

type Captured = (Vec<u8>, OutputTruncation);

#[async_trait]
impl K8sOps for RealK8sOps {
    async fn resource_metadata(
        &self,
        kind: K8sResourceKind,
        namespace: &str,
        name: &str,
    ) -> Result<Option<ObjectMeta>, K8sOpsError> {
        Ok(crate::k8s_ownership_cluster::inspect_resource_metadata(&self.client, kind, namespace, name).await?)
    }

    async fn delete_resource(
        &self,
        receipt: &K8sResourceReceipt,
        resource_version: Option<&str>,
    ) -> Result<(), K8sOpsError> {
        Ok(crate::k8s_ownership_cluster::delete_exact_resource(&self.client, receipt, resource_version).await?)
    }

    async fn set_resource_owner(
        &self,
        receipt: &K8sResourceReceipt,
        owner: &K8sResourceReceipt,
    ) -> Result<(), K8sOpsError> {
        Ok(crate::k8s_ownership_cluster::set_exact_resource_owner(&self.client, receipt, owner).await?)
    }

    async fn put_owned_secret(&self, owner: &K8sResourceReceipt, secret: Secret) -> Result<Secret, K8sOpsError> {
        Ok(crate::k8s_ownership_cluster::put_exact_owned_secret(&self.client, owner, secret).await?)
    }

    async fn apply_owned_network_policy(
        &self,
        owner: &K8sResourceReceipt,
        previous: Option<&K8sResourceReceipt>,
        policy: Option<NetworkPolicy>,
    ) -> Result<Option<NetworkPolicy>, K8sOpsError> {
        Ok(crate::k8s_ownership_cluster::apply_exact_network_policy(&self.client, owner, previous, policy).await?)
    }

    async fn create_job(&self, namespace: &str, job: Job) -> Result<Job, K8sOpsError> {
        let api: Api<Job> = Api::namespaced(self.client.clone(), namespace);
        api.create(&PostParams::default(), &job)
            .await
            .map_err(|error| match error {
                kube::Error::Api(status) if status.code == 409 => K8sOpsError::JobAlreadyExists(status.message),
                other => K8sOpsError::from(other),
            })
    }

    async fn create_pod(&self, namespace: &str, pod: Pod) -> Result<Pod, K8sOpsError> {
        Ok(self.pods(namespace).create(&PostParams::default(), &pod).await?)
    }

    async fn create_secret(&self, namespace: &str, secret: Secret) -> Result<Secret, K8sOpsError> {
        let api: Api<Secret> = Api::namespaced(self.client.clone(), namespace);
        Ok(api.create(&PostParams::default(), &secret).await?)
    }

    async fn put_secret(&self, namespace: &str, secret: Secret) -> Result<(), K8sOpsError> {
        let api: Api<Secret> = Api::namespaced(self.client.clone(), namespace);
        let name = secret.metadata.name.clone().unwrap_or_default();
        match api.replace(&name, &PostParams::default(), &secret).await {
            Ok(_) => Ok(()),
            Err(kube::Error::Api(status)) if status.code == 404 => {
                api.create(&PostParams::default(), &secret).await?;
                Ok(())
            }
            Err(error) => Err(error.into()),
        }
    }

    async fn read_secret_key(&self, namespace: &str, name: &str, key: &str) -> Result<Option<Vec<u8>>, K8sOpsError> {
        let api: Api<Secret> = Api::namespaced(self.client.clone(), namespace);
        Ok(api
            .get_opt(name)
            .await?
            .and_then(|secret| secret.data)
            .and_then(|mut data| data.remove(key))
            .map(|bytes| bytes.0))
    }

    async fn create_network_policy(
        &self,
        namespace: &str,
        policy: NetworkPolicy,
    ) -> Result<NetworkPolicy, K8sOpsError> {
        let api: Api<NetworkPolicy> = Api::namespaced(self.client.clone(), namespace);
        Ok(api.create(&PostParams::default(), &policy).await?)
    }

    async fn create_config_map(&self, namespace: &str, config_map: ConfigMap) -> Result<ConfigMap, K8sOpsError> {
        let api: Api<ConfigMap> = Api::namespaced(self.client.clone(), namespace);
        Ok(api.create(&PostParams::default(), &config_map).await?)
    }

    async fn delete_job(&self, namespace: &str, name: &str) -> Result<(), K8sOpsError> {
        let api: Api<Job> = Api::namespaced(self.client.clone(), namespace);
        deleted_or_missing(api.delete(name, &job_delete_params()).await)
    }

    async fn delete_pod(&self, namespace: &str, name: &str) -> Result<(), K8sOpsError> {
        deleted_or_missing(self.pods(namespace).delete(name, &immediate_delete_params()).await)
    }

    async fn delete_secret(&self, namespace: &str, name: &str) -> Result<(), K8sOpsError> {
        let api: Api<Secret> = Api::namespaced(self.client.clone(), namespace);
        deleted_or_missing(api.delete(name, &immediate_delete_params()).await)
    }

    async fn delete_network_policy(&self, namespace: &str, name: &str) -> Result<(), K8sOpsError> {
        let api: Api<NetworkPolicy> = Api::namespaced(self.client.clone(), namespace);
        deleted_or_missing(api.delete(name, &immediate_delete_params()).await)
    }

    async fn set_secret_owner(&self, namespace: &str, name: &str, owner: OwnerReference) -> Result<(), K8sOpsError> {
        let api: Api<Secret> = Api::namespaced(self.client.clone(), namespace);
        let patch = Patch::Merge(OwnerReferencesPatch::new(owner));
        api.patch(name, &PatchParams::default(), &patch).await?;
        Ok(())
    }

    async fn find_pod_for_job(&self, namespace: &str, job_name: &str) -> Result<String, K8sOpsError> {
        let lp = ListParams::default().labels(&format!("job-name={job_name}"));
        let pods = self.pods(namespace).list(&lp).await?;
        pods.items
            .first()
            .and_then(|pod| pod.metadata.name.clone())
            .ok_or_else(|| K8sOpsError::NoPodForJob {
                job_name: job_name.to_string(),
            })
    }

    async fn list_pods(&self, namespace: &str, label_selector: &str) -> Result<PodListing, K8sOpsError> {
        let (pods, clock) = self
            .list_with_clock(&format!("/api/v1/namespaces/{namespace}/pods"), label_selector)
            .await?;
        Ok(PodListing { pods, clock })
    }

    async fn list_jobs(&self, namespace: &str, label_selector: &str) -> Result<JobListing, K8sOpsError> {
        let (jobs, clock) = self
            .list_with_clock(&format!("/apis/batch/v1/namespaces/{namespace}/jobs"), label_selector)
            .await?;
        Ok(JobListing { jobs, clock })
    }

    async fn patch_pod_labels(
        &self,
        namespace: &str,
        name: &str,
        labels: BTreeMap<String, String>,
        resource_version: &str,
    ) -> Result<FencedPatchOutcome, K8sOpsError> {
        // `metadata.resourceVersion` inside a merge patch is the apiserver's
        // optimistic-concurrency precondition: a stale value is rejected with
        // 409 instead of silently overwriting whoever wrote in between.
        let patch = Patch::Merge(FencedLabelsPatch::new(resource_version, labels));
        match self.pods(namespace).patch(name, &PatchParams::default(), &patch).await {
            Ok(_) => Ok(FencedPatchOutcome::Applied),
            // Discriminated on the typed reason, not the code alone: 409 also
            // carries `AlreadyExists`, which means something else entirely.
            Err(kube::Error::Api(status)) if status.code == 409 && status.reason == "Conflict" => {
                Ok(FencedPatchOutcome::PreconditionFailed)
            }
            Err(error) => Err(error.into()),
        }
    }

    async fn patch_config_map(
        &self,
        namespace: &str,
        name: &str,
        data: BTreeMap<String, String>,
    ) -> Result<(), K8sOpsError> {
        let api: Api<ConfigMap> = Api::namespaced(self.client.clone(), namespace);
        let patch = Patch::Merge(ConfigMapDataPatch::new(data));
        api.patch(name, &PatchParams::default(), &patch).await?;
        Ok(())
    }

    async fn patch_network_policy(
        &self,
        namespace: &str,
        name: &str,
        spec: NetworkPolicySpec,
    ) -> Result<(), K8sOpsError> {
        let api: Api<NetworkPolicy> = Api::namespaced(self.client.clone(), namespace);
        let patch = Patch::Merge(NetworkPolicySpecPatch::new(&spec));
        match api.patch(name, &PatchParams::default(), &patch).await {
            Ok(_) => Ok(()),
            Err(kube::Error::Api(status)) if status.code == 404 => {
                let policy = NetworkPolicy {
                    metadata: ObjectMeta {
                        name: Some(name.to_string()),
                        namespace: Some(namespace.to_string()),
                        ..Default::default()
                    },
                    spec: Some(spec),
                };
                api.create(&PostParams::default(), &policy).await?;
                Ok(())
            }
            Err(error) => Err(error.into()),
        }
    }

    async fn exec_collect_stdout(
        &self,
        namespace: &str,
        pod_name: &str,
        container: &str,
        command: Vec<String>,
    ) -> Result<Vec<u8>, K8sOpsError> {
        let params = AttachParams::default().stdout(true).stderr(true).container(container);
        let mut attached = self.start_exec(namespace, pod_name, &command, &params).await?;

        let mut stdout = require_stream(attached.stdout(), "stdout")?;
        let mut out = Vec::new();
        stdout.read_to_end(&mut out).await.map_err(exec_io("read", "stdout"))?;

        let mut stderr = require_stream(attached.stderr(), "stderr")?;
        let mut err_buf = Vec::new();
        if stderr.read_to_end(&mut err_buf).await.is_ok() && !err_buf.is_empty() {
            let msg = String::from_utf8_lossy(&err_buf);
            tracing::debug!(pod = pod_name, stderr = %msg, "exec stderr");
        }

        attached.join().await.map_err(exec_join_error)?;

        Ok(out)
    }

    async fn exec_with_output(
        &self,
        namespace: &str,
        pod_name: &str,
        container: &str,
        command: Vec<String>,
        stdin_data: Option<Vec<u8>>,
        max_output_bytes: usize,
    ) -> Result<PodExecOutput, K8sOpsError> {
        let params = AttachParams::default()
            .stdin(stdin_data.is_some())
            .stdout(true)
            .stderr(true)
            .container(container);
        let mut attached = self.start_exec(namespace, pod_name, &command, &params).await?;

        if let Some(data) = stdin_data {
            let mut stdin = require_stream(attached.stdin(), "stdin")?;
            stdin.write_all(&data).await.map_err(exec_io("write", "stdin"))?;
            stdin.shutdown().await.map_err(exec_io("close", "stdin"))?;
        }

        let stdout = require_stream(attached.stdout(), "stdout")?;
        let stderr = require_stream(attached.stderr(), "stderr")?;
        let status_fut = require_stream(attached.take_status(), "status")?;
        let ((stdout_buf, stdout_truncated), (stderr_buf, stderr_truncated)) = tokio::try_join!(
            async {
                read_capped(stdout, max_output_bytes)
                    .await
                    .map_err(exec_io("read", "stdout"))
            },
            async {
                read_capped(stderr, max_output_bytes)
                    .await
                    .map_err(exec_io("read", "stderr"))
            },
        )?;

        attached.join().await.map_err(exec_join_error)?;
        let status = status_fut.await;

        Ok(PodExecOutput {
            exit_code: exit_code_from_status(status.as_ref()),
            stdout: stdout_buf,
            stderr: stderr_buf,
            truncated: OutputTruncation::from(stdout_truncated.is_truncated() || stderr_truncated.is_truncated()),
            ..Default::default()
        })
    }

    async fn exec_interactive(
        &self,
        namespace: &str,
        pod_name: &str,
        container: &str,
        command: Vec<String>,
        retention: OutputRetention,
        max_output_bytes: usize,
    ) -> Result<InteractivePodExec, K8sOpsError> {
        let params = AttachParams::default()
            .stdin(true)
            .stdout(true)
            .stderr(true)
            .container(container);
        let mut attached = self.start_exec(namespace, pod_name, &command, &params).await?;
        let stdin = require_stream(attached.stdin(), "stdin")?;
        let mut stdout = require_stream(attached.stdout(), "stdout")?;
        let stderr = require_stream(attached.stderr(), "stderr")?;
        let status_fut = require_stream(attached.take_status(), "status")?;

        let completion = async move {
            let read_stdout = async {
                match retention {
                    OutputRetention::Buffer => read_capped(stdout, max_output_bytes)
                        .await
                        .map_err(exec_io("read", "stdout")),
                    OutputRetention::StreamOnly => {
                        tokio::io::copy(&mut stdout, &mut tokio::io::sink())
                            .await
                            .map_err(exec_io("read", "stdout"))?;
                        Ok(Captured::default())
                    }
                }
            };
            let read_stderr = async {
                match retention {
                    OutputRetention::Buffer => read_capped(stderr, max_output_bytes)
                        .await
                        .map_err(exec_io("read", "stderr")),
                    OutputRetention::StreamOnly => read_tail(stderr, OutputRetention::STREAM_ONLY_STDERR_TAIL_BYTES)
                        .await
                        .map_err(exec_io("read", "stderr")),
                }
            };
            let ((stdout_buf, stdout_truncated), (stderr_buf, stderr_truncated)) =
                tokio::try_join!(read_stdout, read_stderr)?;

            attached.join().await.map_err(exec_join_error)?;
            let status = status_fut.await;

            Ok(PodExecOutput {
                exit_code: exit_code_from_status(status.as_ref()),
                stdout: stdout_buf,
                stderr: stderr_buf,
                truncated: OutputTruncation::from(stdout_truncated.is_truncated() || stderr_truncated.is_truncated()),
                ..Default::default()
            })
        };

        Ok(InteractivePodExec {
            stdin: Box::pin(stdin),
            completion: Box::pin(completion),
        })
    }

    async fn exec_tty(
        &self,
        namespace: &str,
        pod_name: &str,
        container: &str,
        command: Vec<String>,
        size: (u16, u16),
    ) -> Result<TtyPodExec, K8sOpsError> {
        // With a TTY the apiserver rejects a separate stderr stream.
        let params = AttachParams::default()
            .stdin(true)
            .stdout(true)
            .stderr(false)
            .tty(true)
            .container(container);
        let mut attached = self.start_exec(namespace, pod_name, &command, &params).await?;
        let stdin = require_stream(attached.stdin(), "stdin")?;
        let stdout = require_stream(attached.stdout(), "stdout")?;
        let mut size_tx = require_stream(attached.terminal_size(), "terminal size")?;
        let status_fut = require_stream(attached.take_status(), "status")?;

        let (resize, mut resize_rx) = mpsc::channel::<(u16, u16)>(TTY_RESIZE_QUEUE);
        // The queue is empty and has room, so the initial size always fits.
        resize.try_send(size).ok();
        tokio::spawn(async move {
            while let Some((cols, rows)) = resize_rx.recv().await {
                let sent = size_tx
                    .send(TerminalSize {
                        width: cols,
                        height: rows,
                    })
                    .await;
                if sent.is_err() {
                    break;
                }
            }
        });

        let completion = async move {
            attached.join().await.map_err(exec_join_error)?;
            let status = status_fut.await;
            Ok(exit_code_from_status(status.as_ref()))
        };

        Ok(TtyPodExec {
            stdin: Box::pin(stdin),
            stdout: Box::pin(stdout),
            resize,
            completion: Box::pin(completion),
        })
    }

    fn spawn_job_watcher(
        &self,
        namespace: &str,
        name: &str,
        tx: watch::Sender<SandboxStatus>,
    ) -> tokio::task::JoinHandle<()> {
        let client = self.client.clone();
        let namespace = namespace.to_string();
        let name = name.to_string();

        tokio::spawn(async move {
            let api: Api<Job> = Api::namespaced(client, &namespace);
            let cfg = WatcherConfig::default().fields(&format!("metadata.name={name}"));
            let mut stream = std::pin::pin!(watcher::watcher(api, cfg));

            while let Some(event) = stream.next().await {
                let phase = match event {
                    Ok(watcher::Event::Apply(job)) | Ok(watcher::Event::InitApply(job)) => job_phase_from_k8s(&job),
                    Ok(watcher::Event::Delete(_)) => {
                        let _ = tx.send(SandboxStatus::Cancelled);
                        return;
                    }
                    Ok(watcher::Event::Init) | Ok(watcher::Event::InitDone) => continue,
                    Err(_) => continue,
                };

                let status = match phase {
                    JobPhase::Succeeded => SandboxStatus::Completed,
                    JobPhase::Failed { reason } => SandboxStatus::Failed { error: reason },
                    JobPhase::Running => SandboxStatus::Running,
                    JobPhase::Pending => SandboxStatus::Pending,
                };

                if tx.send(status).is_err() {
                    return;
                }
            }
        })
    }
}
