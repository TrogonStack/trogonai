//! Token vault for AI-provider API key proxying.
//!
//! Services hold opaque tokens (`tok_anthropic_prod_abc`) instead of real API
//! keys. The vault stores and resolves these tokens to the actual credentials.
//!
//! # Quick Start
//!
//! ```rust
//! use trogon_vault::{ApiKeyToken, MemoryVault, VaultStore};
//!
//! # #[tokio::main]
//! # async fn main() {
//! let vault = MemoryVault::new();
//! let token = ApiKeyToken::new("tok_anthropic_prod_a1b2c3").unwrap();
//!
//! vault.store(&token, "sk-ant-realkey-...").await.unwrap();
//!
//! let key = vault.resolve(&token).await.unwrap();
//! assert_eq!(key, Some("sk-ant-realkey-...".to_string()));
//! # }
//! ```

pub mod backends;
pub mod token;
pub mod vault;

pub use backends::memory::{MemoryVault, MemoryVaultError};
pub use token::{AiProvider, ApiKeyToken, Env, TokenError};
pub use vault::VaultStore;
