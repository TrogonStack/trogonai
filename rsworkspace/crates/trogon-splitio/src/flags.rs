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

use std::collections::HashMap;
use serde_json::Value;

use crate::{CONTROL, SplitClient, error::SplitError};

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

impl SplitClient {
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
        self.get_treatment_or_control(key, flag.name(), attributes).await == "on"
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
    fn name(&self) -> &'static str { self.0 }
}

/// A feature flag that is always `"off"` — useful in tests.
pub struct AlwaysOff(pub &'static str);

impl FeatureFlag for AlwaysOff {
    fn name(&self) -> &'static str { self.0 }
}

/// The treatment returned for [`CONTROL`] — the safe default when a flag
/// is unknown or the evaluator is unavailable.
pub const CONTROL_TREATMENT: &str = CONTROL;

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::MockServer;
    use crate::{SplitClient, SplitConfig};

    // Sample flag enum — exactly like Spacedrive's pattern
    enum AppFlag {
        NewDashboard,
        BetaSearch,
    }

    impl FeatureFlag for AppFlag {
        fn name(&self) -> &'static str {
            match self {
                Self::NewDashboard => "new_dashboard",
                Self::BetaSearch   => "beta_search",
            }
        }
    }

    fn make_client(base_url: &str) -> SplitClient {
        SplitClient::new(SplitConfig {
            evaluator_url: base_url.to_string(),
            auth_token: "tok".to_string(),
        })
    }

    // ── is_enabled ────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn is_enabled_returns_true_for_on_treatment() {
        let server = MockServer::start_async().await;
        server.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/client/get-treatment")
                .query_param("split-name", "new_dashboard");
            then.status(200).json_body(serde_json::json!({ "treatment": "on" }));
        });

        let client = make_client(&server.base_url());
        assert!(client.is_enabled("user-1", &AppFlag::NewDashboard, None).await);
    }

    #[tokio::test]
    async fn is_enabled_returns_false_for_off_treatment() {
        let server = MockServer::start_async().await;
        server.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/client/get-treatment")
                .query_param("split-name", "new_dashboard");
            then.status(200).json_body(serde_json::json!({ "treatment": "off" }));
        });

        let client = make_client(&server.base_url());
        assert!(!client.is_enabled("user-1", &AppFlag::NewDashboard, None).await);
    }

    #[tokio::test]
    async fn is_enabled_returns_false_for_control() {
        let server = MockServer::start_async().await;
        server.mock(|when, then| {
            when.method(httpmock::Method::GET).path("/client/get-treatment");
            then.status(200).json_body(serde_json::json!({ "treatment": "control" }));
        });

        let client = make_client(&server.base_url());
        assert!(!client.is_enabled("user-1", &AppFlag::NewDashboard, None).await);
    }

    #[tokio::test]
    async fn is_enabled_returns_false_on_network_error() {
        let client = make_client("http://127.0.0.1:1");
        assert!(!client.is_enabled("user-1", &AppFlag::NewDashboard, None).await);
    }

    #[tokio::test]
    async fn is_enabled_uses_typed_flag_name() {
        let server = MockServer::start_async().await;
        // Must be called with "beta_search", not "new_dashboard"
        let mock = server.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/client/get-treatment")
                .query_param("split-name", "beta_search");
            then.status(200).json_body(serde_json::json!({ "treatment": "on" }));
        });

        let client = make_client(&server.base_url());
        client.is_enabled("u", &AppFlag::BetaSearch, None).await;
        mock.assert_async().await;
    }

    // ── all_enabled ───────────────────────────────────────────────────────────

    #[tokio::test]
    async fn all_enabled_returns_true_when_all_on() {
        let server = MockServer::start_async().await;
        server.mock(|when, then| {
            when.method(httpmock::Method::GET).path("/client/get-treatment");
            then.status(200).json_body(serde_json::json!({ "treatment": "on" }));
        });

        let client = make_client(&server.base_url());
        let flags: Vec<&dyn FeatureFlag> = vec![&AppFlag::NewDashboard, &AppFlag::BetaSearch];
        assert!(client.all_enabled("u", &flags, None).await);
    }

    #[tokio::test]
    async fn all_enabled_returns_false_when_one_off() {
        let server = MockServer::start_async().await;
        // First call returns "on", second returns "off"
        server.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/client/get-treatment")
                .query_param("split-name", "new_dashboard");
            then.status(200).json_body(serde_json::json!({ "treatment": "on" }));
        });
        server.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/client/get-treatment")
                .query_param("split-name", "beta_search");
            then.status(200).json_body(serde_json::json!({ "treatment": "off" }));
        });

        let client = make_client(&server.base_url());
        let flags: Vec<&dyn FeatureFlag> = vec![&AppFlag::NewDashboard, &AppFlag::BetaSearch];
        assert!(!client.all_enabled("u", &flags, None).await);
    }

    #[tokio::test]
    async fn all_enabled_empty_list_returns_true() {
        let client = make_client("http://127.0.0.1:1"); // no network needed
        assert!(client.all_enabled("u", &[], None).await);
    }

    // ── any_enabled ───────────────────────────────────────────────────────────

    #[tokio::test]
    async fn any_enabled_returns_true_when_one_on() {
        let server = MockServer::start_async().await;
        server.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/client/get-treatment")
                .query_param("split-name", "new_dashboard");
            then.status(200).json_body(serde_json::json!({ "treatment": "off" }));
        });
        server.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/client/get-treatment")
                .query_param("split-name", "beta_search");
            then.status(200).json_body(serde_json::json!({ "treatment": "on" }));
        });

        let client = make_client(&server.base_url());
        let flags: Vec<&dyn FeatureFlag> = vec![&AppFlag::NewDashboard, &AppFlag::BetaSearch];
        assert!(client.any_enabled("u", &flags, None).await);
    }

    #[tokio::test]
    async fn any_enabled_returns_false_when_all_off() {
        let server = MockServer::start_async().await;
        server.mock(|when, then| {
            when.method(httpmock::Method::GET).path("/client/get-treatment");
            then.status(200).json_body(serde_json::json!({ "treatment": "off" }));
        });

        let client = make_client(&server.base_url());
        let flags: Vec<&dyn FeatureFlag> = vec![&AppFlag::NewDashboard, &AppFlag::BetaSearch];
        assert!(!client.any_enabled("u", &flags, None).await);
    }

    #[tokio::test]
    async fn any_enabled_empty_list_returns_false() {
        let client = make_client("http://127.0.0.1:1");
        assert!(!client.any_enabled("u", &[], None).await);
    }

    // ── treatment_for ─────────────────────────────────────────────────────────

    #[tokio::test]
    async fn treatment_for_returns_raw_treatment() {
        let server = MockServer::start_async().await;
        server.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/client/get-treatment")
                .query_param("split-name", "new_dashboard");
            then.status(200).json_body(serde_json::json!({ "treatment": "blue" }));
        });

        let client = make_client(&server.base_url());
        let t = client.treatment_for("u", &AppFlag::NewDashboard, None).await.unwrap();
        assert_eq!(t, "blue");
    }

    // ── description default ───────────────────────────────────────────────────

    #[test]
    fn description_defaults_to_name() {
        assert_eq!(AppFlag::NewDashboard.description(), AppFlag::NewDashboard.name());
    }
}
