use async_nats::Message;

use crate::verb_context::VerbContext;
use crate::{
    K8sVerb, account_verbs, command_verbs, file_verbs, network_verbs, snapshot_verbs, ssh_verbs, terminal_verb,
};

/// Answers a verb for a lease this worker holds. Anything for another lease is left to the worker that owns it.
pub(crate) async fn handle_verb(ctx: &VerbContext, verb: K8sVerb, msg: Message) {
    match verb {
        K8sVerb::NetworkPolicyUpdate => network_verbs::network_policy_update(ctx, msg).await,
        K8sVerb::CheckpointRunning => snapshot_verbs::checkpoint_running(ctx, msg).await,
        K8sVerb::Rollback => snapshot_verbs::rollback(ctx, msg).await,
        K8sVerb::RunCommand => command_verbs::run_command(ctx, msg).await,
        K8sVerb::PollCommand => command_verbs::poll_command(ctx, msg).await,
        K8sVerb::KillCommand => command_verbs::kill_command(ctx, msg).await,
        K8sVerb::StopCommand => command_verbs::stop_command(ctx, msg).await,
        K8sVerb::RestartCommand => command_verbs::restart_command(ctx, msg).await,
        K8sVerb::WriteStdin => command_verbs::write_stdin(ctx, msg).await,
        K8sVerb::WriteFiles => file_verbs::write_files(ctx, msg).await,
        K8sVerb::ReadFile => file_verbs::read_file(ctx, msg).await,
        K8sVerb::FileOp => file_verbs::file_op(ctx, msg).await,
        K8sVerb::CreateUser => account_verbs::create_user(ctx, msg).await,
        K8sVerb::CreateGroup => account_verbs::create_group(ctx, msg).await,
        K8sVerb::ModifyGroupMembership => account_verbs::modify_group_membership(ctx, msg).await,
        K8sVerb::GetDomain => terminal_verb::get_domain(ctx, msg).await,
        K8sVerb::Terminal => terminal_verb::terminal(ctx, msg).await,
        K8sVerb::SshAttach => ssh_verbs::ssh_attach(ctx, msg).await,
        K8sVerb::SshDetach => ssh_verbs::ssh_detach(ctx, msg).await,
    }
}
