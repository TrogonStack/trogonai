# trogon_lints

Repository-owned Rust policy lints for Trogon.

This Dylint crate is intentionally isolated from the parent Cargo workspace and
pins its compiler in `rust-toolchain.toml`. The nightly toolchain is only for
building the rustc-integrated lint library; the main Rust workspace keeps using
its normal toolchain.

## Rules

Each rule's default level is declared in `src/lib.rs`, so policy lives in the
lint crate rather than in per-invocation flags.

- `constant_outside_constants_module` (`deny`): requires module-level (and
  crate-root) `const` items to live in a `constants` module (`constants.rs`), so
  a module's tunable values are discoverable in one place instead of scattered
  across the modules that use them. A crate may have more than one (a crate-root
  `constants.rs` plus a nested `constants.rs` per submodule group). Constants
  inside a
  function body are local implementation details and are left alone; associated
  consts (`impl`/`trait`) are not free items and are never considered; `static`
  items are out of scope. Test and benchmark sources (`tests.rs`, `*_tests.rs`,
  anything under a Cargo `tests/` or `benches/` directory, and inline
  `tests`/`benches` modules and the not-for-prod test-support module family
  (`test_support`, `mocks`, `fixtures`, `testkit`, `*_harness`)) carry fixtures
  rather than crate configuration and are exempt, as are generated files (those carrying an `@generated` marker near
  the top, e.g. proto codegen); suppress a justified exception with
  `#[cfg_attr(dylint_lib = "trogon_lints", allow(constant_outside_constants_module))]`
  at the site.
- `error_string_comparison` (`deny`): prevents semantic checks against strings
  derived from `std::error::Error::to_string`.
- `function_local_use` (`deny`): requires `use` imports to live at module level
  rather than inside a function body or block. A function-local import is never
  required (every name is reachable by full path or a module-level `use`, with
  `as` for collisions) and it hides a module's dependency surface inside its
  functions. Macro-generated imports (from expansion) and `@generated` files
  (proto codegen, etc.) are exempt; suppress a justified exception with
  `#[cfg_attr(dylint_lib = "trogon_lints", allow(function_local_use))]` at the
  site.
- `manual_error_impl` (`deny`): requires deriving `std::error::Error` with
  `thiserror` instead of hand-writing the impl.
- `inline_module_block` (`deny`): requires modules to live in their own file
  (`mod foo;`) instead of inline blocks (`mod foo { ... }`). Macro-generated
  modules and `@generated` files (proto codegen, etc.) are exempt; suppress a
  justified exception with
  `#[cfg_attr(dylint_lib = "trogon_lints", allow(inline_module_block))]` at the
  site. As a late (HIR) pass it sees `#[cfg(test)] mod tests { ... }` only when
  the test target is compiled, i.e. when linting with `--all-targets`.
- `serde_json_macro` (`deny`): requires JSON payloads to be built from a named
  type (`#[derive(serde::Serialize)]` plus `serde_json::to_value`) instead of an
  ad-hoc `serde_json::json!` literal, so every payload the code emits has a
  name, a schema, and a single definition to change. Fires once per hand-written
  invocation, however it is spelled (`json!`, `serde_json::json!`, or nested in
  another macro call). Test and benchmark sources (`tests.rs`, `*_tests.rs`,
  anything under a Cargo `tests/` or `benches/` directory, `#[cfg(test)]` and
  `#[test]` code, inline `tests`/`benches` modules and the not-for-prod
  test-support module family (`test_support`, `mocks`, `fixtures`, `testkit`,
  `*_harness`)) build fixtures rather than production payloads and are exempt,
  as are generated files (those carrying an `@generated` marker near the top);
  a genuinely dynamic shape (an upstream document passed through verbatim, a
  payload whose keys are decided at runtime) opts out at the site with
  `#[cfg_attr(dylint_lib = "trogon_lints", allow(serde_json_macro, reason = "..."))]`,
  where the `reason` records the technical justification and is required by
  `serde_json_macro_allow_without_reason`. As a late (HIR) pass it sees
  `#[cfg(test)] mod tests { ... }` only when the test target is compiled, i.e.
  when linting with `--all-targets`.
- `serde_json_macro_allow_without_reason` (`deny`): requires an
  `allow(serde_json_macro)` or `expect(serde_json_macro)` to carry a non-empty
  `reason = "..."`. Rust accepts a bare `allow`, so without this the escape
  hatch records nothing and a silenced diagnostic is indistinguishable from an
  argued exception. Runs as an early (AST) pass, since lint level attributes
  never reach HIR.
- `std_env_access` (`deny`): requires reading environment variables through the
  injected `trogon_std::env` abstraction (the `ReadEnv` lookup trait and the
  `EnumerateEnv` enumeration trait, backed by `SystemEnv` in production and
  `InMemoryEnv` in tests) rather than calling a `std::env` reader (`var`,
  `var_os`, `vars`, `vars_os`) directly. A direct call couples logic to
  process-global state that cannot be supplied deterministically in a test.
  `trogon-std`'s own `SystemEnv` is the one allowed caller and is exempt;
  suppress a justified exception with
  `#[cfg_attr(dylint_lib = "trogon_lints", allow(std_env_access))]` at the site.
- `unstructured_log_fields` (`deny`): requires a `tracing` event macro (`info!`,
  `warn!`, `error!`, `debug!`, `trace!`, `event!`) to record its values as
  structured fields rather than interpolate them all into the message. A field
  is a typed key-value pair a subscriber can filter, index, and forward as a
  column; a value baked into the message text is reachable only by regular
  expression, and loses the name that ties it to the same value logged
  elsewhere. A message with no placeholders captures nothing and has nothing to
  structure. Fields the callsite already carries do not excuse it, because a
  field covers whatever it names and not the separate value spliced into the
  text. `target:` and `parent:` are directives rather than fields and do not
  count as structuring. Test sources (`tests.rs`, `*_tests.rs`) are exempt;
  suppress a justified exception with
  `#[cfg_attr(dylint_lib = "trogon_lints", allow(unstructured_log_fields))]` at
  the site.

## Credits

`unstructured_log_fields` is ported from the lint of the same name in
[li-kai/rust-lints](https://github.com/li-kai/rust-lints), documented at
[`docs/unstructured-log-fields.md`](https://github.com/li-kai/rust-lints/blob/main/docs/unstructured-log-fields.md).
The rule, its name, and the shape of its exceptions are theirs; full credit for
the idea goes to li-kai. The implementation here was written against this
crate's own helpers rather than copied, because the upstream repository
publishes no license.

It departs from upstream in one respect. Upstream documents that it "does not
fire when at least one structured field is present"; this port fires whenever
the message interpolates a value, however many fields sit beside it. Upstream's
rule reads a callsite as either structured or not, so `info!(user_id, "user
performed {}", action)` counts as structured and `action` goes unreported even
though only `user_id` is queryable. Treating the fields as a per-value question
instead of a per-callsite one is what lets the rule reach that case.

## Run

From `rsworkspace/` (the `deny` rules are enforced by their declared default
level, no flags needed). This mirrors CI:

```bash
env -u RUSTUP_TOOLCHAIN cargo dylint --path dylints/trogon_lints --workspace --no-deps -- --all-features
```

Add `--all-targets` to also lint test targets such as
`#[cfg(test)] mod tests { ... }`, which a late (HIR) pass only sees when the
test target is compiled:

```bash
env -u RUSTUP_TOOLCHAIN cargo dylint --path dylints/trogon_lints --workspace --no-deps -- --all-features --all-targets
```
