use std::io::{self, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use a2a_auth_callout::jwt::ExternalSubject;
use a2a_auth_callout::{
    AccountName, CallerId, CallerJwtHeaderValue, IssuedPermissions, KeyVersion, NkeyPublic, SigningKeySource,
    SpiceDbPrincipal, StaticSigningKeySource, UserJwtClaims, UserJwtSubject,
};
use a2a_gateway::caller_jwt_header::CALLER_JWT_HEADER_NAME;
use a2a_gateway::ingress_identity::{
    ForwardedCallerJwtVerifier, caller_jwt_issuer_keys_from_env, gateway_jwt_audience,
};
use async_nats::HeaderMap;
use trogon_identity_types::{PrincipalKind, SubjectPrincipal};
use trogon_std::env::{InMemoryEnv, ReadEnv};

#[derive(Debug, thiserror::Error)]
enum FixtureError {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error("bootstrap failed; credential-bearing output suppressed")]
    Bootstrap,
    #[error("bootstrap did not return the public configuration")]
    Configuration,
}

fn bootstrap(seed: &str) -> Result<InMemoryEnv, FixtureError> {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../../devops/docker/compose/services/nats")
        .canonicalize()?;
    let mut child = Command::new("docker")
        .args(["run", "--rm", "-i", "--network", "none", "--user", "0"])
        .args(["--tmpfs", "/secrets", "--tmpfs", "/tmp", "--mount"])
        .arg(format!(
            "type=bind,source={},target=/bootstrap,readonly",
            directory.display()
        ))
        .args(["--entrypoint", "/bin/sh", "natsio/nats-box:0.20.1", "-eu", "-c"])
        .arg(
            "umask 077
             mkdir -p /secrets/clients/auth-callout/keys
             cat > /secrets/clients/auth-callout/keys/tenant-dev.nk
             sh /bootstrap/bootstrap.sh > /tmp/bootstrap.log 2>&1
             cat /secrets/clients/a2a-gateway/gateway.env
             . /secrets/clients/auth-callout/callout.env
             printf 'CALLOUT_AUDIENCE=%s\\n' \"${AUTH_CALLOUT_TENANT_SIGNING_KEYS%%=*}\"",
        )
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let input = child.stdin.take().ok_or(FixtureError::Configuration)?;
    let written = {
        let mut input = input;
        writeln!(input, "{seed}")
    };
    let output = child.wait_with_output()?;
    written?;
    if !output.status.success() {
        return Err(FixtureError::Bootstrap);
    }
    let public_config = String::from_utf8(output.stdout).map_err(|_| FixtureError::Configuration)?;
    let env = InMemoryEnv::new();
    for line in public_config.lines() {
        let (key, value) = line.split_once('=').ok_or(FixtureError::Configuration)?;
        env.set(key, value);
    }
    Ok(env)
}

#[test]
fn bootstrap_configuration_verifies_callout_tokens_and_rejects_other_audiences()
-> Result<(), Box<dyn std::error::Error>> {
    let seed = nkeys::KeyPair::new_account().seed()?;
    let source = StaticSigningKeySource::new(&seed, KeyVersion::new("current")?)?;
    let env = bootstrap(&seed)?;
    let verifier = ForwardedCallerJwtVerifier::new(
        Arc::new(caller_jwt_issuer_keys_from_env(&env)?),
        gateway_jwt_audience(&env, "a2a"),
    );
    let caller = CallerId::new("alice")?;
    let principal = SubjectPrincipal::from_tokens("person", "alice")?;
    let user = UserJwtSubject::from_user_nkey(NkeyPublic::parse(nkeys::KeyPair::new_user().public_key())?);
    let mut claims = UserJwtClaims {
        kid: KeyVersion::new("current")?,
        sub: ExternalSubject::new("alice")?,
        aud: AccountName::new(env.var("CALLOUT_AUDIENCE").map_err(|_| FixtureError::Configuration)?),
        data: SpiceDbPrincipal::new("user/alice"),
        nats_permissions: IssuedPermissions::default_for_caller(&caller),
        caller_id: caller,
        principal_kind: PrincipalKind::Person,
    };
    for accepted in [true, false] {
        if !accepted {
            claims.aud = AccountName::new("other-account");
        }
        let minted = claims.mint(
            &source.current().minting_material(),
            &user,
            SystemTime::now(),
            Duration::from_secs(300),
        )?;
        let header = CallerJwtHeaderValue::from_minted(&minted);
        let mut headers = HeaderMap::new();
        headers.insert(CALLER_JWT_HEADER_NAME, header.as_str());
        assert_eq!(verifier.check(&headers, &principal).is_ok(), accepted);
    }
    Ok(())
}
