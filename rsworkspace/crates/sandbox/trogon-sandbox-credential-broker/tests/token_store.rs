use std::error::Error;
use std::num::NonZeroU32;
use std::time::Duration;

use trogon_sandbox_contract::{CredentialError, CredentialId, CredentialScope, ShortLivedToken, TokenId};
use trogon_sandbox_credential_broker::{BackendError, InMemoryTokenStore, TokenStore, TokenStoreError};

fn scope() -> CredentialScope {
    CredentialScope::RegistryPull {
        repository_glob: "r/*".to_string(),
    }
}

async fn issue(store: &InMemoryTokenStore, ttl: u32) -> Result<TokenId, Box<dyn Error>> {
    let response = store
        .issue(
            &CredentialId::new("c"),
            &scope(),
            ShortLivedToken::new("v"),
            NonZeroU32::new(ttl).ok_or("ttl")?,
        )
        .await?;
    assert_eq!(response.token.expose_secret(), "v");
    Ok(response.token_id)
}

#[tokio::test]
async fn a_token_validates_until_revoked() -> Result<(), Box<dyn Error>> {
    let store = InMemoryTokenStore::new();
    let id = issue(&store, 300).await?;
    let record = store.validate(&id).await?;
    assert_eq!(record.credential_id, CredentialId::new("c"));
    assert_eq!(record.scope, scope());
    store.revoke(&id).await?;
    assert!(matches!(
        store.validate(&id).await,
        Err(CredentialError::TokenRevoked { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn token_ids_are_unique() -> Result<(), Box<dyn Error>> {
    let store = InMemoryTokenStore::new();
    assert_ne!(issue(&store, 300).await?, issue(&store, 300).await?);
    Ok(())
}

#[tokio::test]
async fn a_token_expires_after_its_ttl() -> Result<(), Box<dyn Error>> {
    let store = InMemoryTokenStore::new();
    let id = issue(&store, 1).await?;
    tokio::time::sleep(Duration::from_millis(1100)).await;
    assert!(matches!(
        store.validate(&id).await,
        Err(CredentialError::TokenExpired { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn an_unknown_token_is_not_found_and_revoking_it_is_fine() -> Result<(), Box<dyn Error>> {
    let store = InMemoryTokenStore::new();
    let unknown = TokenId::new("nope");
    assert!(matches!(
        store.validate(&unknown).await,
        Err(CredentialError::NotFound { .. })
    ));
    store.revoke(&unknown).await?;
    Ok(())
}

#[test]
fn token_store_errors_explain_themselves() -> Result<(), Box<dyn Error>> {
    let codec = TokenStoreError::from(serde_json::from_str::<u8>("x").err().ok_or("parsed")?);
    assert_eq!(codec.to_string(), "token record could not be encoded or decoded");
    let backend = TokenStoreError::from(BackendError::new(std::io::Error::other("down")));
    assert_eq!(backend.to_string(), "backend failed: down");
    let rejected = TokenStoreError::Rejected(CredentialError::Backend {
        message: "x".to_string(),
    });
    assert_eq!(
        rejected.to_string(),
        "token could not be validated: credential backend failed: x"
    );
    Ok(())
}
