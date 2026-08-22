# trogon_lints

Repository-owned Rust policy lints for Trogon.

This Dylint crate is intentionally isolated from the parent Cargo workspace and
pins its compiler in `rust-toolchain.toml`. The nightly toolchain is only for
building the rustc-integrated lint library; the main Rust workspace keeps using
its normal toolchain.

## Rules

Each rule's default level is declared in `src/lib.rs`, so policy lives in the
lint crate rather than in per-invocation flags.

- `acyclic_modules` (`deny`): requires the dependencies between sibling modules
  to form a directed acyclic graph, at every depth of the module tree. A cycle
  means neither module can be read, moved, or tested without the other, and it
  is never introduced deliberately: it accrues one reasonable edge at a time
  until two modules are a single unit spelled as two. Sibling graphs aggregate
  every reference made anywhere in a child's subtree, so a cycle distributed
  across grandchildren (`payments::checkout` to `server::auth`,
  `server::routes` back to `payments::billing`) is reported at the level where
  it closes. Parent-child references (a parent declaring and re-exporting its
  children, a child reaching back through `super::`) are how the module tree is
  built rather than how it is layered, and are excluded by construction. Only
  intra-crate references count, since Cargo already forbids cycles between
  crates; macro-generated references are the macro author's and are exempt, as
  is test and benchmark code (`#[test]` functions, `#[cfg(test)]` items, Cargo
  `tests/`/`benches/` targets, `tests.rs`/`*_tests.rs` sources, and the
  test-support module family (`tests`, `benches`, `test_support`, `mocks`,
  `fixtures`, `testkit`, `*_harness`)), which reaches across the tree by design.
  A deliberate coupling opts out on the module that owns both siblings with
  `#[cfg_attr(dylint_lib = "trogon_lints", expect(acyclic_modules, reason = "..."))]`,
  where `expect` reports itself once the cycle is gone so the exemption cannot
  outlive its reason.
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
- `debug_remnants` (`deny`): requires diagnostics to be recorded as `tracing`
  events rather than written to the process's own stdout or stderr with
  `println!`, `print!`, `eprintln!`, or `dbg!`. Those writes carry no level, no
  fields, and no span context, and they bypass the subscriber entirely, so they
  never reach the backend the service exports its logs to; `dbg!` also prints
  the expression's source text and keeps evaluating it in production. `eprint!`
  is not reported, because a write to stderr with no newline is how a terminal
  program draws a prompt or a progress line. The invocation has to be
  hand-written: a macro of someone else's that happens to print is left alone.
  Test and benchmark sources (`tests.rs`, `*_tests.rs`, anything under a Cargo
  `tests/` or `benches/` directory, `#[cfg(test)]` and `#[test]` code, inline
  `tests`/`benches` modules and the not-for-prod test-support module family
  (`test_support`, `mocks`, `fixtures`, `testkit`, `*_harness`)) are exempt,
  where printing is how a failing case explains itself. A write that is
  genuinely the program's own output (a CLI printing its result, a process
  reporting a fatal error before a subscriber exists) opts out at the site with
  `#[cfg_attr(dylint_lib = "trogon_lints", allow(debug_remnants, reason = "..."))]`.
- `error_string_comparison` (`deny`): prevents semantic checks against strings
  derived from `std::error::Error::to_string`.
- `fallible_new` (`deny`): requires a constructor named `new` (or a `new_*`
  variant) not to panic. `new` is the name Rust reserves for construction that
  cannot fail, so a caller reading it has no `Err` to match and nothing for `?`
  to carry: a `.unwrap()`, `.expect(...)`, `panic!`, or `unreachable!` reached
  through that name takes the process down for a failure the caller was never
  offered. Return `Result<Self, _>` (renaming to `try_new` when an infallible
  constructor stays beside it), or move the fallible work out to the caller.
  `todo!` and `unimplemented!` are left to rustc's own lints for unfinished
  code. A constructor that already returns `Result` or `Option` has said
  construction can fail and is left alone, as is one in a trait impl, where the
  implementor owns neither the name nor the return type, and one in a test or
  benchmark source, where a panic fails the test that caused it rather than
  surprising a caller. A panic that is an invariant the caller cannot break opts
  out at the site with
  `#[cfg_attr(dylint_lib = "trogon_lints", allow(fallible_new, reason = "..."))]`.
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
- `unbounded_channel` (`deny`): requires a channel to be created with an
  explicit capacity: `std::sync::mpsc::sync_channel`,
  `tokio::sync::mpsc::channel`, `futures::channel::mpsc::channel`,
  `flume::bounded`, `crossbeam::channel::bounded`, or `async_channel::bounded`,
  rather than their unbounded counterparts. An unbounded queue turns a
  throughput mismatch into a memory leak: the producer never learns it is ahead,
  because the send always succeeds, so the queue grows for as long as the
  imbalance lasts and the first symptom is the process being killed for its
  resident size. A capacity makes the mismatch survivable, since a full queue
  makes the sender wait, and that backpressure reaches through the producer to
  whatever it reads from (a socket stops being drained, a JetStream consumer
  stops acking). The bound also names the worst-case memory the queue can hold,
  which an unbounded channel leaves as a property of the workload. Test and
  benchmark sources are exempt, where a test drives both ends of its channel;
  a queue bounded by something other than its capacity opts out at the site with
  `#[cfg_attr(dylint_lib = "trogon_lints", allow(unbounded_channel, reason = "..."))]`.
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

`unstructured_log_fields`, `acyclic_modules`, `fallible_new`,
`debug_remnants`, and `unbounded_channel` are ported from the lints of the same
names in [li-kai/rust-lints](https://github.com/li-kai/rust-lints), documented at
[`docs/unstructured-log-fields.md`](https://github.com/li-kai/rust-lints/blob/main/docs/unstructured-log-fields.md),
[`docs/acyclic-modules.md`](https://github.com/li-kai/rust-lints/blob/main/docs/acyclic-modules.md),
[`docs/fallible-new.md`](https://github.com/li-kai/rust-lints/blob/main/docs/fallible-new.md),
[`docs/debug-remnants.md`](https://github.com/li-kai/rust-lints/blob/main/docs/debug-remnants.md),
and
[`docs/unbounded-channel.md`](https://github.com/li-kai/rust-lints/blob/main/docs/unbounded-channel.md).
The rules, their names, and the shape of their exceptions are theirs; full
credit for the ideas goes to li-kai. The implementations here were written
against this crate's own helpers rather than copied, because the upstream
repository publishes no license.

`unstructured_log_fields` departs from upstream in one respect. Upstream
documents that it "does not fire when at least one structured field is
present"; this port fires whenever the message interpolates a value, however
many fields sit beside it. Upstream's rule reads a callsite as either
structured or not, so `info!(user_id, "user performed {}", action)` counts as
structured and `action` goes unreported even though only `user_id` is
queryable. Treating the fields as a per-value question instead of a
per-callsite one is what lets the rule reach that case.

`acyclic_modules` departs from upstream in where the diagnostic is levelled.
Upstream documents `#[expect(acyclic_modules, reason = "...")]` as the opt-out
but reports the cycle after the crate is walked, where only a crate-level
attribute is in scope. This port attributes each cycle to the module that owns
both siblings, so the attribute goes on that module and covers the cycle
wherever the individual references sit.

`fallible_new` departs from upstream in two respects. Upstream exposes a
`check_new_variants` configuration flag; this port has no configuration and
always covers `new_*` variants, since a variant constructor makes the same
promise as `new` and a per-repository switch would only let one crate opt out of
the rule the rest follow. Upstream also reports only `Result` as the fallible
return type; this port treats `Option` the same way, because a constructor
returning `Option<Self>` has already told the caller construction can fail.

`debug_remnants` departs from upstream in two respects. Upstream exposes a
`suggested_strategy` configuration flag choosing between `tracing` and `log`;
this port has no configuration and always suggests `tracing`, which is the one
logging facade this repository uses, so the flag would only offer a way to
suggest something the codebase does not do. Upstream also reports only the
outermost expansion node's macro by name, which leaves `dbg!` unreported on a
toolchain where it delegates to an internal macro; this port walks the
expansion chain back out to the invocation a reader can see, so the diagnostic
names `dbg!` and points at the line that was typed.

`unbounded_channel` departs from upstream in two respects. Upstream exempts
channels created in `fn main()`; this port does not, because a queue wired up
during composition grows the same way as one wired up anywhere else, and `main`
is where a long-lived service's channels are usually built. Upstream also
exposes an `additional_paths` configuration flag for naming further
constructors; this port has no configuration and carries the full set it
recognises in the lint crate, extended beyond upstream's list with
`futures::channel::mpsc::unbounded` and `async_channel::unbounded`.

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

## Prior art

Several of these lints were prompted by [li-kai/rust-lints](https://github.com/li-kai/rust-lints),
which arrived at the same policies independently and first: `acyclic_modules`,
`debug_remnants`, `fallible_new`, `unbounded_channel`, and `unstructured_log_fields`
all have counterparts there, and the names are taken from it. The implementations
here are our own, written against a different matching strategy, but the choice of
what to enforce owes that crate the credit.
