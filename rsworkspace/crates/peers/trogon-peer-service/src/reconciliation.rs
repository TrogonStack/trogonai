use crate::{
    content_binding::ContentBinding,
    content_ports::ContentOpen,
    peer_access::PeerAccess,
    peer_application::{ApplicationResult, PeerApplication},
    peer_application_error::PeerApplicationError,
    peer_clock::PeerClock,
    peer_operation::PeerOperation,
    scoped_peer_store::ScopedPeerStore,
    work_limit::WorkLimit,
};
use trogon_decider_runtime::{CommandAuthorizer, CommandError, StreamAppend, StreamRead};
use trogon_peer::{
    AcceptedMessage, AuthoritativeReceipt, DisposeMessage, IntakeMessage, MessageError, MessageId, PeerId,
    SettleMessage, TerminalDisposition, TrustedContent, TrustedPeerOwner,
};
use trogonai_proto::convert::datetime_from_timestamp;
impl<S, P, C, Content, Cache, Ids, Catalog> PeerApplication<S, P, C, Content, Cache, Ids, Catalog>
where
    S: StreamRead<PeerId> + StreamAppend<PeerId> + ScopedPeerStore,
    P: CommandAuthorizer<PeerAccess>
        + CommandAuthorizer<IntakeMessage>
        + CommandAuthorizer<DisposeMessage>
        + CommandAuthorizer<SettleMessage>
        + Send
        + Sync,
    C: PeerClock,
    Content: ContentOpen,
{
    /// Processes only the named sender obligations. The count is attempted obligations, not delivery or completion receipts.
    pub async fn reconcile(
        &self,
        owner: &TrustedPeerOwner,
        sender: &PeerId,
        messages: &[MessageId],
        limit: WorkLimit,
    ) -> ApplicationResult<usize, S> {
        self.authorize(owner, sender, PeerOperation::Reconcile)?;
        if messages.is_empty()
            || messages.len() > limit.get()
            || messages.iter().enumerate().any(|(i, id)| messages[..i].contains(id))
        {
            return Err(PeerApplicationError::Input);
        }
        let history = self.load(owner, sender).await?;
        if history.enrollment.caller_id.as_deref() != Some(owner.caller().as_str()) {
            return Err(PeerApplicationError::Scope);
        }
        // Authorize every destination in the finite request before accessing another port.
        let mut obligations = Vec::new();
        for id in messages {
            let evidence = AcceptedMessage::from_sender_history(&history, id)?;
            let recipient = evidence.recipient().clone();
            let scope = self.authorize(owner, &recipient, PeerOperation::ReadPeer)?;
            obligations.push((id, evidence, recipient, scope));
        }
        for (id, evidence, recipient, scope) in obligations {
            let target = self.load(owner, &recipient).await?;
            if target.collaboration() != history.collaboration() {
                return Err(PeerApplicationError::Scope);
            }
            if !target
                .dispositions
                .iter()
                .any(|r| r.message_id.as_deref() == Some(id.as_str()))
            {
                let expiry = datetime_from_timestamp(
                    evidence
                        .metadata()
                        .expires_at
                        .as_option()
                        .ok_or(PeerApplicationError::Scope)?,
                )
                .map_err(|_| PeerApplicationError::Input)?;
                let mut observed_at = self.clock.now();
                let disposition = if observed_at >= expiry {
                    Some(TerminalDisposition::Expired)
                } else {
                    let binding = binding(owner, evidence.content());
                    let opened = self.content.open(&scope, &binding).await?;
                    observed_at = self.clock.now();
                    if observed_at >= expiry {
                        Some(TerminalDisposition::Expired)
                    } else {
                        match opened {
                            Some(text) if text.as_str().len() == binding.content().size() as usize => None,
                            Some(_) => return Err(PeerApplicationError::Scope),
                            None => Some(TerminalDisposition::Unavailable),
                        }
                    }
                };
                if let Some(disposition) = disposition {
                    match self
                        .execute_message(owner, &DisposeMessage::new(evidence, disposition, observed_at))
                        .await
                    {
                        Ok(_)
                        | Err(PeerApplicationError::Message(CommandError::Decide(MessageError::AlreadyDisposed(_)))) => {
                        }
                        Err(error) => return Err(error),
                    }
                } else {
                    match self
                        .execute_message(owner, &IntakeMessage::new(evidence, observed_at))
                        .await
                    {
                        Ok(_)
                        | Err(PeerApplicationError::Message(CommandError::Decide(
                            MessageError::AlreadyIntaken(_) | MessageError::AlreadyDisposed(_),
                        ))) => {}
                        Err(PeerApplicationError::Message(CommandError::Decide(MessageError::DeferIntake))) => continue,
                        Err(error) => return Err(error),
                    }
                }
            }
            let winner = self.load(owner, &recipient).await?;
            if winner
                .dispositions
                .iter()
                .any(|r| r.message_id.as_deref() == Some(id.as_str()))
            {
                let receipt = AuthoritativeReceipt::from_recipient_history(&winner, id)?;
                match self
                    .execute_message(owner, &SettleMessage::new(sender.clone(), receipt))
                    .await
                {
                    Ok(_)
                    | Err(PeerApplicationError::Message(CommandError::Decide(MessageError::AlreadySettled(_)))) => {}
                    Err(error) => return Err(error),
                }
            }
        }
        Ok(messages.len())
    }
}

pub(crate) fn binding(owner: &TrustedPeerOwner, content: &TrustedContent) -> ContentBinding {
    ContentBinding::from_content(owner.tenant().clone(), content.clone())
}
