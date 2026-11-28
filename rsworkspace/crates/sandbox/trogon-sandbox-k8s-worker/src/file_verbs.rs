use async_nats::Message;
use trogon_sandbox_contract::{
    FileOpRequest, FileOpResponse, LeaseError, ReadFileRequest, ReadFileResponse, WriteFilesRequest, WriteFilesResponse,
};
use trogon_sandbox_protocol::{encode_read_file_response_within, nats_transfer, unsupported_file_op_response};

use crate::constants::DEFAULT_FILE_MODE;
use crate::inbound::Inbound;
use crate::shell_wrap::runuser;
use crate::verb_context::VerbContext;
use crate::{K8sVerb, K8sWorkerProvider};

fn quoted(value: &str) -> String {
    value.replace('\'', "'\\''")
}

fn backend(message: String) -> LeaseError {
    LeaseError::Backend { message }
}

pub(crate) async fn write_files(ctx: &VerbContext, msg: Message) {
    let Some(inbound) = Inbound::<WriteFilesRequest>::receive(&ctx.client, &msg, K8sVerb::WriteFiles).await else {
        return;
    };
    if let Some(response) = write_all_files(&ctx.provider, &inbound.request).await {
        ctx.reply_transfer(&inbound.reply, serde_json::to_vec(&response)).await;
    }
}

/// Writes every file in order, stopping at the first failure. `None` when this worker does not hold the lease.
pub(crate) async fn write_all_files(
    provider: &K8sWorkerProvider,
    request: &WriteFilesRequest,
) -> Option<WriteFilesResponse> {
    if !provider.holds_lease(&request.lease_id).await {
        return None;
    }
    let as_user = request.as_user.as_ref().map(|user| user.as_str());
    for file in &request.files {
        let mode = file.mode.unwrap_or(DEFAULT_FILE_MODE);
        let path = quoted(&file.path);
        let script = format!("mkdir -p \"$(dirname '{path}')\" && cat > '{path}' && chmod {mode:o} '{path}'");
        let command = match as_user {
            Some(user) => runuser(user, &script),
            None => vec!["sh".to_owned(), "-c".to_owned(), script],
        };
        let result = provider
            .exec_in_pod_with_stdin_for_lease(&request.lease_id, command, file.content.clone())
            .await?;
        let failure = match result {
            Ok(output) if output.exit_code == 0 => continue,
            Ok(output) => format!(
                "write_files failed for {}: exit code {} ({})",
                file.path,
                output.exit_code,
                String::from_utf8_lossy(&output.stderr)
            ),
            Err(error) => error.to_string(),
        };
        return Some(WriteFilesResponse::Denied(backend(failure)));
    }
    Some(WriteFilesResponse::Written)
}

pub(crate) async fn read_file(ctx: &VerbContext, msg: Message) {
    let Some(inbound) = Inbound::<ReadFileRequest>::receive(&ctx.client, &msg, K8sVerb::ReadFile).await else {
        return;
    };
    let request = inbound.request;
    let command = match &request.as_user {
        Some(user) => vec![
            "sh".to_owned(),
            "-c".to_owned(),
            format!(
                "runuser -u '{}' -- cat '{}'",
                quoted(user.as_str()),
                quoted(&request.path)
            ),
        ],
        None => vec!["cat".to_owned(), request.path.clone()],
    };
    let Some(result) = ctx.provider.exec_in_pod_for_lease(&request.lease_id, command).await else {
        return;
    };
    let response = match result {
        Ok(output) if output.exit_code == 0 => ReadFileResponse::Content { data: output.stdout },
        Ok(output) => ReadFileResponse::Denied(backend(format!(
            "read_file failed for {}: exit code {} ({})",
            request.path,
            output.exit_code,
            String::from_utf8_lossy(&output.stderr)
        ))),
        Err(error) => ReadFileResponse::Denied(backend(error.to_string())),
    };
    let body = encode_read_file_response_within(&response, &request.path, nats_transfer::MAX_TRANSFER_BYTES);
    ctx.reply_transfer(&inbound.reply, body).await;
}

pub(crate) async fn file_op(ctx: &VerbContext, msg: Message) {
    let Some(inbound) = Inbound::<FileOpRequest>::receive(&ctx.client, &msg, K8sVerb::FileOp).await else {
        return;
    };
    let request = inbound.request;
    if ctx.provider.sandbox_for_lease(&request.lease_id).await.is_none() {
        return;
    }
    let response: FileOpResponse = unsupported_file_op_response("k8s_micro_vm", &request.op);
    ctx.reply_transfer(&inbound.reply, serde_json::to_vec(&response)).await;
}
