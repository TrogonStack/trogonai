use trogon_sandbox_contract::{
    AwsCredentialPairBinding, AwsCredentialPairs, SecretBinding, SecretBindings, SecretTarget,
};

pub use crate::constants::{
    CONVENTIONAL_AWS_ACCESS_KEY_ID, CONVENTIONAL_AWS_SECRET_ACCESS_KEY, CONVENTIONAL_AWS_SESSION_TOKEN,
};
use crate::{ConventionalAwsPair, UnpairedReason};

/// Two bindings inject at the same places: same hosts and same request scope.
fn same_destination(a: &SecretBinding, b: &SecretBinding) -> bool {
    a.inject_hosts() == b.inject_hosts() && a.inject_scope() == b.inject_scope()
}

/// Pairs `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY` and the optional `AWS_SESSION_TOKEN` when they are whole-value
/// masked environment bindings, so a SigV4 request signed with the sentinels is re-signed without anyone declaring a
/// pair.
///
/// Declaring a pair that claims any of the three variables turns this off. A member that injects at different hosts
/// than the access key id stays an ordinary binding, so the secret is never swapped anywhere the access key id is
/// not.
pub fn conventional_aws_pair(bindings: &SecretBindings, declared: &AwsCredentialPairs) -> ConventionalAwsPair {
    let whole = |var: &str| {
        bindings.bindings().iter().find(|binding| {
            binding.is_whole_value_mask() && matches!(binding.target(), SecretTarget::Env(name) if name.as_str() == var)
        })
    };
    let access_key_id = whole(CONVENTIONAL_AWS_ACCESS_KEY_ID);
    let secret_access_key = whole(CONVENTIONAL_AWS_SECRET_ACCESS_KEY);
    let session_token = whole(CONVENTIONAL_AWS_SESSION_TOKEN);

    let claimed = [access_key_id, secret_access_key, session_token]
        .into_iter()
        .flatten()
        .any(|binding| {
            declared
                .pairs()
                .iter()
                .any(|pair| pair.members().any(|member| member == binding.name()))
        });
    if claimed {
        return ConventionalAwsPair::default();
    }
    let Some(access_key_id) = access_key_id else {
        return ConventionalAwsPair {
            pair: None,
            unpaired: secret_access_key
                .map(|binding| {
                    (
                        binding.name().clone(),
                        UnpairedReason::SecretAccessKeyWithoutAccessKeyId,
                    )
                })
                .into_iter()
                .collect(),
        };
    };
    let Some(secret_access_key) = secret_access_key else {
        return ConventionalAwsPair::default();
    };
    if !same_destination(secret_access_key, access_key_id) {
        return ConventionalAwsPair {
            pair: None,
            unpaired: vec![(
                secret_access_key.name().clone(),
                UnpairedReason::HostsDifferFromAccessKeyId,
            )],
        };
    }
    let mut unpaired = Vec::new();
    let session_token = session_token.and_then(|binding| {
        if same_destination(binding, access_key_id) {
            Some(binding.name().clone())
        } else {
            unpaired.push((binding.name().clone(), UnpairedReason::HostsDifferFromAccessKeyId));
            None
        }
    });
    let pair = AwsCredentialPairBinding::new(
        access_key_id.name().clone(),
        secret_access_key.name().clone(),
        session_token,
    )
    .ok();
    ConventionalAwsPair { pair, unpaired }
}
