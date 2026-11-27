use trogon_sandbox_contract::{LeaseId, WorkerId};

use crate::SignedSubject;
use crate::constants::{LEASE_VERBS, MAX_READINESS_LEASE_LEN};

/// Classifies a subject, or `None` when it is not a signed request.
pub fn classify_subject(subject: &str) -> Option<SignedSubject> {
    let tokens: Vec<&str> = subject.split('.').collect();
    match tokens.as_slice() {
        ["sandbox", "provider", provider, "native_snapshot_delete"] if !provider.is_empty() => {
            return Some(SignedSubject::NativeSnapshotDelete {
                provider: (*provider).to_owned(),
            });
        }
        ["sandbox", "ssh", "attach" | "detach", lease @ ..]
            if !lease.is_empty() && lease.iter().all(|token| !token.is_empty()) =>
        {
            return Some(SignedSubject::SshSession {
                lease: LeaseId::new(lease.join(".")),
            });
        }
        ["sandbox", "worker", worker, "snapshot", "export"] if !worker.is_empty() => {
            return Some(SignedSubject::SnapshotExport);
        }
        _ => {}
    }
    if tokens.len() < 4 || tokens[0] != "sandbox" || tokens[1] != "provider" {
        return None;
    }
    match tokens.as_slice() {
        [_, _, provider, "worker", worker, "command_readiness", lease @ ..] => {
            command_readiness(provider, worker, lease)
        }
        [_, _, _, "purge_named"] => Some(SignedSubject::PurgeNamed),
        [_, _, _, "worker", _, "cleanup"] => Some(SignedSubject::Cleanup),
        [_, _, _, verb, lease @ ..]
            if LEASE_VERBS.contains(verb) && !lease.is_empty() && lease.iter().all(|token| !token.is_empty()) =>
        {
            Some(SignedSubject::LeaseVerb {
                lease: LeaseId::new(lease.join(".")),
            })
        }
        _ => None,
    }
}

fn command_readiness(provider: &str, worker: &str, lease: &[&str]) -> Option<SignedSubject> {
    let provider_ok = !provider.is_empty()
        && provider
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'));
    let lease_ok = !lease.is_empty()
        && lease.iter().all(|token| {
            !token.is_empty()
                && !token.chars().any(|character| {
                    character.is_control() || character.is_whitespace() || matches!(character, '*' | '>')
                })
        })
        && lease.iter().map(|token| token.len()).sum::<usize>() + lease.len() - 1 <= MAX_READINESS_LEASE_LEN;
    let worker = WorkerId::new(worker).ok()?;
    (provider_ok && lease_ok).then(|| SignedSubject::WorkerCommandReadiness {
        provider: provider.to_owned(),
        worker,
        lease: LeaseId::new(lease.join(".")),
    })
}
