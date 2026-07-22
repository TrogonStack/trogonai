pub mod api_key;
pub mod mtls;
pub mod oidc;

/// The credential source the auth callout received, in preference order.
///
/// Preference: OIDC (primary) → mTLS (service-to-service) → API key (transitional).
/// See docs/a2a/explanation/auth-callout-design.md §2 "Credential sources (preference order)".
pub enum CredentialSource {
    Oidc,
    MTls,
    #[deprecated(note = "transitional only; remove after OIDC migration")]
    ApiKey,
}
