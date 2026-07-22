# trogon-splitio

Feature flag evaluation for Rust using [Split.io / Harness FME](https://www.harness.io/products/feature-management-experimentation).

## Background

Split.io has no official Rust SDK. The recommended path for unsupported languages is to run the **Split Evaluator** as a sidecar service and call it over HTTP. This crate wraps those HTTP calls with an ergonomic, type-safe Rust API.

The developer experience is modelled after [Spacedrive](https://spacedrive.com)'s feature flag pattern: flags are defined as a typed enum in one central place rather than scattered string literals, so the compiler catches typos and refactors are trivial.

## Architecture

```
[your code]
    │
    │  HTTP (reqwest)
    ▼
split-evaluator :7548        ← splitsoftware/split-evaluator Docker image
    │
    │  Split SDK protocol
    ▼
Split.io / Harness FME CDN
```

Your Rust code never holds the server-side SDK key. That credential lives only inside the evaluator container. The Rust client authenticates to the evaluator with a separate `auth_token` of your choosing.

## Setup

### 1. Start the Split Evaluator sidecar

```bash
docker run -p 7548:7548 \
  -e SPLIT_EVALUATOR_API_KEY=<your-harness-sdk-key> \
  -e SPLIT_EVALUATOR_AUTH_TOKEN=<your-auth-token> \
  splitsoftware/split-evaluator:latest
```

- `SPLIT_EVALUATOR_API_KEY` — the server-side SDK key from your Harness FME environment settings
- `SPLIT_EVALUATOR_AUTH_TOKEN` — a token you define; the Rust client sends it as the `Authorization` header

### 2. Configure the Rust client

| Env var | Default | Description |
|---|---|---|
| `SPLIT_EVALUATOR_URL` | `http://localhost:7548` | Base URL of the evaluator |
| `SPLIT_EVALUATOR_AUTH_TOKEN` | *(empty)* | Must match the token configured on the evaluator |

## Usage

### Define your flags (Spacedrive-inspired pattern)

Define all feature flags in one place as an enum. This is the key insight from Spacedrive: a central typed list makes flags discoverable and prevents typos.

```rust
use trogon_splitio::FeatureFlag;

pub enum AppFlag {
    NewCheckoutFlow,
    BetaDashboard,
    ExperimentalSearch,
}

impl FeatureFlag for AppFlag {
    fn name(&self) -> &'static str {
        match self {
            Self::NewCheckoutFlow    => "new_checkout_flow",
            Self::BetaDashboard      => "beta_dashboard",
            Self::ExperimentalSearch => "experimental_search",
        }
    }
}
```

The string returned by `name()` must match the flag name in the Split.io / Harness dashboard.

### Initialise the client

```rust
use trogon_splitio::{SplitClient, SplitConfig};
use trogon_std::env::SystemEnv;

let cfg = SplitConfig::from_env(&SystemEnv);
let client = SplitClient::new(cfg);
```

### Evaluate flags

```rust
// Boolean check — the primary API
if client.is_enabled("user-123", &AppFlag::NewCheckoutFlow, None).await {
    // show new checkout
}

// With targeting attributes
let mut attrs = HashMap::new();
attrs.insert("plan".to_string(), json!("premium"));

if client.is_enabled("user-123", &AppFlag::BetaDashboard, Some(&attrs)).await {
    // show beta dashboard to premium users
}

// Multiple flags — AND
if client.all_enabled("user-123", &[&AppFlag::NewCheckoutFlow, &AppFlag::BetaDashboard], None).await {
    // both flags are on
}

// Multiple flags — OR
if client.any_enabled("user-123", &[&AppFlag::NewCheckoutFlow, &AppFlag::BetaDashboard], None).await {
    // at least one flag is on
}
```

### Never block on flags

Feature flags should never crash your main flow. Use `get_treatment_or_control` (or `is_enabled`, which wraps it) to get a safe fallback on any error:

```rust
// Returns "control" if the evaluator is down — never panics, never errors
let treatment = client
    .get_treatment_or_control("user-123", "my_flag", None)
    .await;
```

### Raw treatment access

When a flag has more than two states (e.g. `"blue"`, `"green"`, `"red"`) or carries a JSON config payload:

```rust
// Raw treatment string
let treatment = client
    .treatment_for("user-123", &AppFlag::NewCheckoutFlow, None)
    .await?;

// Treatment + JSON config payload
let result = client
    .get_treatment_with_config("user-123", "ui_theme", None)
    .await?;

println!("treatment: {}", result.treatment);
if let Some(config) = result.config {
    println!("color: {}", config["color"]);
}
```

### Bulk evaluation

```rust
let map = client
    .get_treatments("user-123", &["flag_a", "flag_b", "flag_c"], None)
    .await?;

for (flag, treatment) in &map {
    println!("{flag}: {treatment}");
}
```

### Event tracking

Track custom events for metric measurement in Split.io experiments:

```rust
client
    .track("user-123", "user", "purchase_completed", Some(49.99), None)
    .await?;
```

### Health check

```rust
if client.is_healthy().await {
    println!("Evaluator is up");
}
```

## Key concepts

| Concept | Description |
|---|---|
| **Treatment** | The value returned for a flag: `"on"`, `"off"`, a custom string, or `"control"` |
| **Control** | Returned when the flag doesn't exist or the evaluator is unavailable. Never gate new behaviour on `"control"`. |
| **User key** | Unique identifier for the entity being evaluated (user ID, account ID, device ID) |
| **Attributes** | Key-value map used for targeting rules (e.g. `{"plan": "premium"}`) |
| **Traffic type** | Category of entity (`"user"`, `"account"`) — must be pre-configured in Harness FME |
| **Config** | Optional JSON bundled with a treatment via `get_treatment_with_config` |

## Inspiration

The `FeatureFlag` trait and `is_enabled` / `all_enabled` / `any_enabled` API are directly inspired by [Spacedrive](https://spacedrive.com)'s feature flag system, which defines flags as a typed constant list and exposes a simple `isEnabled(flag)` function. The Rust equivalent uses a trait so any enum can become a set of feature flags with full compile-time safety.

## Testing

Tests use `httpmock` — no Docker or real evaluator required:

```bash
cargo test -p trogon-splitio
```

43 tests cover all methods, error paths, auth header forwarding, attribute passing, and the typed flag API.
