//! Typed feature flag definitions — inspired by Spacedrive's feature flag pattern.
//!
//! Spacedrive defines feature flags as a typed `const` tuple so flag names can
//! never be misspelled at call sites.  This module brings the same idea to Rust:
//! define all flags in one place as an enum, implement [`FeatureFlag`], and use
//! [`SplitClient::is_enabled`] / [`SplitClient::is_enabled_for`] everywhere.
//!
//! # Defining your flags
//!
//! ```rust
//! use trogon_splitio::flags::FeatureFlag;
//!
//! pub enum MyFlag {
//!     NewCheckoutFlow,
//!     BetaDashboard,
//!     ExperimentalSearch,
//! }
//!
//! impl FeatureFlag for MyFlag {
//!     fn name(&self) -> &'static str {
//!         match self {
//!             Self::NewCheckoutFlow    => "new_checkout_flow",
//!             Self::BetaDashboard      => "beta_dashboard",
//!             Self::ExperimentalSearch => "experimental_search",
//!         }
//!     }
//! }
//! ```
//!
//! # Evaluating flags
//!
//! ```no_run
//! # use trogon_splitio::{SplitClient, SplitConfig};
//! # use trogon_splitio::flags::FeatureFlag;
//! # pub enum MyFlag { NewCheckoutFlow }
//! # impl FeatureFlag for MyFlag { fn name(&self) -> &'static str { "new_checkout_flow" } }
//! # async fn example() {
//! # let client = SplitClient::new(SplitConfig { evaluator_url: "".into(), auth_token: "".into() });
//! // No stringly-typed flag names — the compiler catches typos.
//! if client.is_enabled("user-123", &MyFlag::NewCheckoutFlow, None).await {
//!     // show new checkout
//! }
//! # }
//! ```

use serde_json::Value;
use std::collections::HashMap;

use crate::{CONTROL, SplitClient, client::HttpClient, error::SplitError};

/// A type-safe feature flag definition.
///
/// Implement this trait on an enum to get compile-time flag name safety.
/// The string returned by [`name`] must match the flag name configured in
/// the Split.io / Harness FME dashboard.
///
/// [`name`]: FeatureFlag::name
pub trait FeatureFlag: Send + Sync {
    /// The flag name as configured in Split.io (e.g. `"new_checkout_flow"`).
    fn name(&self) -> &'static str;

    /// Human-readable description shown in debug output.  Defaults to [`name`].
    ///
    /// [`name`]: FeatureFlag::name
    fn description(&self) -> &'static str {
        self.name()
    }
}

impl<H: HttpClient> SplitClient<H> {
    /// Return `true` if the flag treatment is `"on"` for the given user.
    ///
    /// Any treatment other than `"on"` — including `"off"`, `"control"`, or
    /// any custom variant — returns `false`.  Network errors also return `false`.
    ///
    /// This is the primary method to use at call sites:
    ///
    /// ```no_run
    /// # use trogon_splitio::{SplitClient, SplitConfig};
    /// # use trogon_splitio::flags::FeatureFlag;
    /// # enum F { MyFlag }
    /// # impl FeatureFlag for F { fn name(&self) -> &'static str { "f" } }
    /// # async fn ex(client: SplitClient) {
    /// if client.is_enabled("user-123", &F::MyFlag, None).await {
    ///     // flag is on
    /// }
    /// # }
    /// ```
    pub async fn is_enabled(
        &self,
        key: &str,
        flag: &dyn FeatureFlag,
        attributes: Option<&HashMap<String, Value>>,
    ) -> bool {
        self.get_treatment_or_control(key, flag.name(), attributes)
            .await
            == "on"
    }

    /// Return `true` if **all** of the provided flags are `"on"` for the user.
    ///
    /// Short-circuits on the first `false` — remaining flags are not evaluated.
    pub async fn all_enabled(
        &self,
        key: &str,
        flags: &[&dyn FeatureFlag],
        attributes: Option<&HashMap<String, Value>>,
    ) -> bool {
        for flag in flags {
            if !self.is_enabled(key, *flag, attributes).await {
                return false;
            }
        }
        true
    }

    /// Return `true` if **any** of the provided flags is `"on"` for the user.
    pub async fn any_enabled(
        &self,
        key: &str,
        flags: &[&dyn FeatureFlag],
        attributes: Option<&HashMap<String, Value>>,
    ) -> bool {
        for flag in flags {
            if self.is_enabled(key, *flag, attributes).await {
                return true;
            }
        }
        false
    }

    /// Get the treatment for a typed flag (rather than a raw string name).
    ///
    /// Returns the raw treatment string — use [`is_enabled`] for boolean checks.
    ///
    /// [`is_enabled`]: SplitClient::is_enabled
    pub async fn treatment_for(
        &self,
        key: &str,
        flag: &dyn FeatureFlag,
        attributes: Option<&HashMap<String, Value>>,
    ) -> Result<String, SplitError> {
        self.get_treatment(key, flag.name(), attributes).await
    }
}

/// A feature flag that is always `"on"` — useful in tests.
pub struct AlwaysOn(pub &'static str);

impl FeatureFlag for AlwaysOn {
    fn name(&self) -> &'static str {
        self.0
    }
}

/// A feature flag that is always `"off"` — useful in tests.
pub struct AlwaysOff(pub &'static str);

impl FeatureFlag for AlwaysOff {
    fn name(&self) -> &'static str {
        self.0
    }
}

/// The treatment returned for [`CONTROL`] — the safe default when a flag
/// is unknown or the evaluator is unavailable.
pub const CONTROL_TREATMENT: &str = CONTROL;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SplitConfig;
    use crate::client::mock::MockHttpClient;

    // Sample flag enum — exactly like Spacedrive's pattern
    enum AppFlag {
        NewDashboard,
        BetaSearch,
    }

    impl FeatureFlag for AppFlag {
        fn name(&self) -> &'static str {
            match self {
                Self::NewDashboard => "new_dashboard",
                Self::BetaSearch => "beta_search",
            }
        }
    }

    fn make_config() -> SplitConfig {
        SplitConfig {
            evaluator_url: "http://unused".to_string(),
            auth_token: "tok".to_string(),
        }
    }

    // ── is_enabled ────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn is_enabled_returns_true_for_on_treatment() {
        let mock = MockHttpClient::new().enqueue_ok(200, r#"{"treatment":"on"}"#);
        let client = SplitClient::new_with(make_config(), mock);
        assert!(client.is_enabled("user-1", &AppFlag::NewDashboard, None).await);
    }

    #[tokio::test]
    async fn is_enabled_returns_false_for_off_treatment() {
        let mock = MockHttpClient::new().enqueue_ok(200, r#"{"treatment":"off"}"#);
        let client = SplitClient::new_with(make_config(), mock);
        assert!(!client.is_enabled("user-1", &AppFlag::NewDashboard, None).await);
    }

    #[tokio::test]
    async fn is_enabled_returns_false_for_control() {
        let mock = MockHttpClient::new().enqueue_ok(200, r#"{"treatment":"control"}"#);
        let client = SplitClient::new_with(make_config(), mock);
        assert!(!client.is_enabled("user-1", &AppFlag::NewDashboard, None).await);
    }

    #[tokio::test]
    async fn is_enabled_returns_false_on_network_error() {
        let mock = MockHttpClient::new().enqueue_err("connection refused");
        let client = SplitClient::new_with(make_config(), mock);
        assert!(!client.is_enabled("user-1", &AppFlag::NewDashboard, None).await);
    }

    #[tokio::test]
    async fn is_enabled_uses_typed_flag_name() {
        // BetaSearch.name() == "beta_search" — the flag name drives the evaluator query
        let mock = MockHttpClient::new().enqueue_ok(200, r#"{"treatment":"on"}"#);
        let client = SplitClient::new_with(make_config(), mock);
        assert!(client.is_enabled("u", &AppFlag::BetaSearch, None).await);
    }

    // ── all_enabled ───────────────────────────────────────────────────────────

    #[tokio::test]
    async fn all_enabled_returns_true_when_all_on() {
        let mock = MockHttpClient::new()
            .enqueue_ok(200, r#"{"treatment":"on"}"#)
            .enqueue_ok(200, r#"{"treatment":"on"}"#);
        let client = SplitClient::new_with(make_config(), mock);
        let flags: Vec<&dyn FeatureFlag> = vec![&AppFlag::NewDashboard, &AppFlag::BetaSearch];
        assert!(client.all_enabled("u", &flags, None).await);
    }

    #[tokio::test]
    async fn all_enabled_returns_false_when_one_off() {
        // NewDashboard → "on", BetaSearch → "off"
        let mock = MockHttpClient::new()
            .enqueue_ok(200, r#"{"treatment":"on"}"#)
            .enqueue_ok(200, r#"{"treatment":"off"}"#);
        let client = SplitClient::new_with(make_config(), mock);
        let flags: Vec<&dyn FeatureFlag> = vec![&AppFlag::NewDashboard, &AppFlag::BetaSearch];
        assert!(!client.all_enabled("u", &flags, None).await);
    }

    #[tokio::test]
    async fn all_enabled_empty_list_returns_true() {
        let client = SplitClient::new_with(make_config(), MockHttpClient::new());
        assert!(client.all_enabled("u", &[], None).await);
    }

    // ── any_enabled ───────────────────────────────────────────────────────────

    #[tokio::test]
    async fn any_enabled_returns_true_when_one_on() {
        // NewDashboard → "off", BetaSearch → "on"
        let mock = MockHttpClient::new()
            .enqueue_ok(200, r#"{"treatment":"off"}"#)
            .enqueue_ok(200, r#"{"treatment":"on"}"#);
        let client = SplitClient::new_with(make_config(), mock);
        let flags: Vec<&dyn FeatureFlag> = vec![&AppFlag::NewDashboard, &AppFlag::BetaSearch];
        assert!(client.any_enabled("u", &flags, None).await);
    }

    #[tokio::test]
    async fn any_enabled_returns_false_when_all_off() {
        let mock = MockHttpClient::new()
            .enqueue_ok(200, r#"{"treatment":"off"}"#)
            .enqueue_ok(200, r#"{"treatment":"off"}"#);
        let client = SplitClient::new_with(make_config(), mock);
        let flags: Vec<&dyn FeatureFlag> = vec![&AppFlag::NewDashboard, &AppFlag::BetaSearch];
        assert!(!client.any_enabled("u", &flags, None).await);
    }

    #[tokio::test]
    async fn any_enabled_empty_list_returns_false() {
        let client = SplitClient::new_with(make_config(), MockHttpClient::new());
        assert!(!client.any_enabled("u", &[], None).await);
    }

    // ── treatment_for ─────────────────────────────────────────────────────────

    #[tokio::test]
    async fn treatment_for_returns_raw_treatment() {
        let mock = MockHttpClient::new().enqueue_ok(200, r#"{"treatment":"blue"}"#);
        let client = SplitClient::new_with(make_config(), mock);
        let t = client
            .treatment_for("u", &AppFlag::NewDashboard, None)
            .await
            .unwrap();
        assert_eq!(t, "blue");
    }

    // ── custom variant → is_enabled = false ──────────────────────────────────

    /// Any treatment that is not exactly `"on"` must return `false` from
    /// `is_enabled`.  Custom variants (`"blue"`, `"v2"`, etc.) are not
    /// considered enabled — only `"on"` is.
    #[tokio::test]
    async fn is_enabled_returns_false_for_custom_variant() {
        let mock = MockHttpClient::new().enqueue_ok(200, r#"{"treatment":"blue"}"#);
        let client = SplitClient::new_with(make_config(), mock);
        assert!(
            !client.is_enabled("u", &AppFlag::NewDashboard, None).await,
            "custom variant 'blue' must not be treated as enabled"
        );
    }

    // ── short-circuit behavior ────────────────────────────────────────────────

    /// `all_enabled` must stop evaluating after the first `false` flag.
    /// Only one response is queued — MockHttpClient panics if a second call
    /// is made, which would fail the test if short-circuiting is broken.
    #[tokio::test]
    async fn all_enabled_short_circuits_after_first_false() {
        let mock = MockHttpClient::new()
            .enqueue_ok(200, r#"{"treatment":"off"}"#); // NewDashboard → off; BetaSearch must not be queried
        let client = SplitClient::new_with(make_config(), mock);
        let flags: Vec<&dyn FeatureFlag> = vec![&AppFlag::NewDashboard, &AppFlag::BetaSearch];
        assert!(!client.all_enabled("u", &flags, None).await);
    }

    /// `any_enabled` must stop evaluating after the first `true` flag.
    /// Only one response is queued — MockHttpClient panics if a second call
    /// is made, which would fail the test if short-circuiting is broken.
    #[tokio::test]
    async fn any_enabled_short_circuits_after_first_true() {
        let mock = MockHttpClient::new()
            .enqueue_ok(200, r#"{"treatment":"on"}"#); // NewDashboard → on; BetaSearch must not be queried
        let client = SplitClient::new_with(make_config(), mock);
        let flags: Vec<&dyn FeatureFlag> = vec![&AppFlag::NewDashboard, &AppFlag::BetaSearch];
        assert!(client.any_enabled("u", &flags, None).await);
    }

    // ── description default ───────────────────────────────────────────────────

    #[test]
    fn description_defaults_to_name() {
        assert_eq!(
            AppFlag::NewDashboard.description(),
            AppFlag::NewDashboard.name()
        );
    }
}
