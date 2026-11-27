use super::*;

/// Handles a broadcast terminal request. Routed like
/// [`handle_write_stdin_msg`]: only the worker holding `lease_id` acts and
/// replies.
pub(super) async fn handle_terminal_msg(
    msg: async_nats::Message,
    client: Client,
    provider: Arc<DockerWorkerProvider>,
    active: ActiveLeases,
    terminals: Arc<DockerTerminals>,
) -> anyhow::Result<()> {
    let Some(reply) = msg.reply.clone() else {
        return Ok(());
    };
    let request: trogon_sandbox_contract::TerminalRequest = match serde_json::from_slice(&msg.payload) {
        Ok(r) => r,
        Err(e) => {
            warn!(error = %e, "invalid terminal request");
            return Ok(());
        }
    };
    if !active.lock().await.contains_key(&request.lease_id) {
        return Ok(());
    }
    let response = terminals.handle(&provider, &client, &msg.payload).await;
    client
        .publish(reply, Bytes::from(serde_json::to_vec(&response)?))
        .await?;
    Ok(())
}

/// Handles a broadcast write_files request. Routed like `run_command`.
pub(super) async fn handle_write_files_msg(
    msg: async_nats::Message,
    client: Client,
    provider: Arc<DockerWorkerProvider>,
    active: ActiveLeases,
) -> anyhow::Result<()> {
    let Some(reply) = msg.reply.clone() else {
        return Ok(());
    };

    let payload = trogon_sandbox_protocol::nats_transfer::receive(&client, &msg).await?;
    let request: WriteFilesRequest = match serde_json::from_slice(&payload) {
        Ok(r) => r,
        Err(e) => {
            warn!(error = %e, "invalid write_files request");
            return Ok(());
        }
    };

    if !active.lock().await.contains_key(&request.lease_id) {
        return Ok(());
    }

    let response = match provider
        .write_files_for_lease(&request.lease_id, &request.files, request.as_user.as_ref())
        .await
    {
        Ok(()) => WriteFilesResponse::Written,
        Err(e) => WriteFilesResponse::Denied(LeaseError::Backend { message: e.to_string() }),
    };

    let body = serde_json::to_vec(&response)?;
    trogon_sandbox_protocol::nats_transfer::respond(&client, reply.as_str(), Bytes::from(body)).await?;
    Ok(())
}

/// Handles a broadcast read_file request. Routed like `run_command`.
pub(super) async fn handle_read_file_msg(
    msg: async_nats::Message,
    client: Client,
    provider: Arc<DockerWorkerProvider>,
    active: ActiveLeases,
) -> anyhow::Result<()> {
    let Some(reply) = msg.reply.clone() else {
        return Ok(());
    };

    let payload = trogon_sandbox_protocol::nats_transfer::receive(&client, &msg).await?;
    let request: ReadFileRequest = match serde_json::from_slice(&payload) {
        Ok(r) => r,
        Err(e) => {
            warn!(error = %e, "invalid read_file request");
            return Ok(());
        }
    };

    if !active.lock().await.contains_key(&request.lease_id) {
        return Ok(());
    }

    let response = trogon_sandbox_protocol::read_file_response(
        provider
            .read_file_for_lease(&request.lease_id, &request.path, request.as_user.as_ref())
            .await,
        |e| LeaseError::Backend { message: e.to_string() },
    );

    let body = super::encode_within::encode_read_within(
        response,
        &request.path,
        trogon_sandbox_protocol::nats_transfer::MAX_TRANSFER_BYTES,
    )?;
    trogon_sandbox_protocol::nats_transfer::respond(&client, reply.as_str(), Bytes::from(body)).await?;
    Ok(())
}

/// Handles a broadcast file_op request. Routed like [`handle_read_file_msg`]; only the worker
/// holding the lease's container answers.
pub(super) async fn handle_file_op_msg(
    msg: async_nats::Message,
    client: Client,
    provider: Arc<DockerWorkerProvider>,
    active: ActiveLeases,
) -> anyhow::Result<()> {
    let Some(reply) = msg.reply.clone() else {
        return Ok(());
    };

    let payload = trogon_sandbox_protocol::nats_transfer::receive(&client, &msg).await?;
    let request: trogon_sandbox_contract::FileOpRequest = match serde_json::from_slice(&payload) {
        Ok(r) => r,
        Err(e) => {
            warn!(error = %e, "invalid file_op request");
            return Ok(());
        }
    };

    if !active.lock().await.contains_key(&request.lease_id) {
        return Ok(());
    }

    let response = provider.file_op_for_lease(&request).await.unwrap_or_else(|| {
        trogon_sandbox_contract::FileOpResponse::Denied(LeaseError::Backend {
            message: format!("no active container for lease {}", request.lease_id),
        })
    });

    let body = super::encode_within::encode_file_op_within(
        response,
        trogon_sandbox_protocol::nats_transfer::MAX_TRANSFER_BYTES,
    )?;
    trogon_sandbox_protocol::nats_transfer::respond(&client, reply.as_str(), Bytes::from(body)).await?;
    Ok(())
}

/// Handles a broadcast create_user request. Routed like `run_command`.
pub(super) async fn handle_create_user_msg(
    msg: async_nats::Message,
    client: Client,
    provider: Arc<DockerWorkerProvider>,
    active: ActiveLeases,
) -> anyhow::Result<()> {
    let Some(reply) = msg.reply else {
        return Ok(());
    };

    let request: CreateSandboxUserRequest = match serde_json::from_slice(&msg.payload) {
        Ok(r) => r,
        Err(e) => {
            warn!(error = %e, "invalid create_user request");
            return Ok(());
        }
    };

    if !active.lock().await.contains_key(&request.lease_id) {
        return Ok(());
    }

    let response = match provider
        .create_user_for_lease(&request.lease_id, &request.username)
        .await
    {
        Ok(response) => response,
        Err(e) => {
            trogon_sandbox_contract::CreateSandboxUserResponse::Denied(LeaseError::Backend { message: e.to_string() })
        }
    };

    let body = serde_json::to_vec(&response)?;
    client.publish(reply, Bytes::from(body)).await?;
    Ok(())
}

/// Handles a broadcast create_group request. Routed like `run_command`.
pub(super) async fn handle_create_group_msg(
    msg: async_nats::Message,
    client: Client,
    provider: Arc<DockerWorkerProvider>,
    active: ActiveLeases,
) -> anyhow::Result<()> {
    let Some(reply) = msg.reply else {
        return Ok(());
    };

    let request: CreateSandboxGroupRequest = match serde_json::from_slice(&msg.payload) {
        Ok(r) => r,
        Err(e) => {
            warn!(error = %e, "invalid create_group request");
            return Ok(());
        }
    };

    if !active.lock().await.contains_key(&request.lease_id) {
        return Ok(());
    }

    let response = match provider
        .create_group_for_lease(&request.lease_id, &request.groupname)
        .await
    {
        Ok(response) => response,
        Err(e) => {
            trogon_sandbox_contract::CreateSandboxGroupResponse::Denied(LeaseError::Backend { message: e.to_string() })
        }
    };

    let body = serde_json::to_vec(&response)?;
    client.publish(reply, Bytes::from(body)).await?;
    Ok(())
}

/// Handles a broadcast modify_group_membership request. Routed like
/// `run_command`.
pub(super) async fn handle_modify_group_membership_msg(
    msg: async_nats::Message,
    client: Client,
    provider: Arc<DockerWorkerProvider>,
    active: ActiveLeases,
) -> anyhow::Result<()> {
    let Some(reply) = msg.reply else {
        return Ok(());
    };

    let request: ModifyGroupMembershipRequest = match serde_json::from_slice(&msg.payload) {
        Ok(r) => r,
        Err(e) => {
            warn!(error = %e, "invalid modify_group_membership request");
            return Ok(());
        }
    };

    if !active.lock().await.contains_key(&request.lease_id) {
        return Ok(());
    }

    let response = match provider
        .modify_group_membership_for_lease(&request.lease_id, &request.username, &request.groupname, request.action)
        .await
    {
        Ok(response) => response,
        Err(e) => trogon_sandbox_contract::ModifyGroupMembershipResponse::Denied(LeaseError::Backend {
            message: e.to_string(),
        }),
    };

    let body = serde_json::to_vec(&response)?;
    client.publish(reply, Bytes::from(body)).await?;
    Ok(())
}
