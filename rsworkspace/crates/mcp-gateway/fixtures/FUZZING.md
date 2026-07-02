# Fuzzing the mcp-gateway scanner

This crate has no `cargo-fuzz`/`libFuzzer` harness. This document records
how one would be wired up, as the WI-06 counterpart to AGT's
`agent-governance-python/fuzz/fuzz_mcp_security.py` ClusterFuzzLite target
(MIT license; see `fixtures/LICENSE-MIT`), without actually adding a fuzz
crate to this workspace.

## Why no fuzz crate is added here

`cargo-fuzz` requires nightly Rust and its own Cargo workspace member
(conventionally `fuzz/`, excluded from the parent workspace via
`[workspace]` in its own `Cargo.toml` so `cargo build`/`cargo test` at the
top level never need nightly). Adding that is a build-topology decision
affecting CI and toolchain pinning for the whole `rsworkspace`, out of
scope for a fixtures-only work item. No `fuzz/` directory or cargo-fuzz
config exists elsewhere in this repository today (checked
`rsworkspace/wasm-components` and the workspace root; neither has one), so
there is also no existing convention to follow.

## What a harness would target

Every detector in `mcp-gateway::scan` is a pure function over owned value
types (`ToolName`, `ServerName`, `&str` descriptions) with no I/O, so each
is fuzzable directly with arbitrary byte input decoded to a `String`,
mirroring AGT's Python target's `data.decode("utf-8", errors="replace")`
step:

- `scan::hidden_instructions::check_hidden_instructions(description, tool_name, server_name)`
- `scan::description_injection::check_description_injection(description, tool_name, server_name)`
- `scan::typosquat::levenshtein_distance(a, b)` and `check_cross_server`
- `scan::rug_pull::check_rug_pull`
- `scan::schema_drift::compare` / `compare_tool`

The two highest-value targets are `check_hidden_instructions` and
`check_description_injection`: both parse untrusted, attacker-controlled
tool-description text (the same MCP `tools/list` response field AGT's
Python `MCPSecurityScanner.scan_tool` fuzzes) through hand-rolled
byte/char scanning logic (UTF-8 boundary walks, a hand-rolled base64
decoder, ordered phrase matching) with no `regex` crate backing it, which
is exactly the kind of manual parsing where off-by-one panics or infinite
loops hide.

## Concrete harness sketch

If/when a `fuzz/` member is added (its own `Cargo.toml` with
`cargo-fuzz` + `libfuzzer-sys`, excluded from the main workspace), the
target would look like:

```rust
// fuzz/fuzz_targets/hidden_instructions.rs
#![no_main]
use libfuzzer_sys::fuzz_target;
use mcp_gateway::{ServerName, ToolName};
use mcp_gateway::scan::hidden_instructions::check_hidden_instructions;

fuzz_target!(|data: &[u8]| {
    let Ok(description) = std::str::from_utf8(data) else {
        return;
    };
    let tool_name = ToolName::new("fuzz-tool").expect("valid");
    let server_name = ServerName::new("fuzz-server").expect("valid");

    // The only correctness property fuzzed here is "never panics, never
    // hangs": check_hidden_instructions has no documented invariant beyond
    // returning a (possibly empty) Vec<McpThreat> for any input string.
    let _ = check_hidden_instructions(description, &tool_name, &server_name);
});
```

A second target for `check_description_injection` follows the identical
shape. Both would run under `cargo +nightly fuzz run hidden_instructions`
once the `fuzz/` member exists, with a corpus seeded from
`fixtures/prompt-injection/injection-smoke.jsonl`'s `text` fields (already
vendored in this crate) to bias the fuzzer toward realistic inputs before
it explores further.

## What this buys over the benchmark tests

`tests/prompt_injection_benchmark.rs` and `tests/redteam_asi_benchmark.rs`
measure detection quality (recall/false-positive rate) on fixed, labelled
corpora. A fuzz harness targets a different property: that arbitrary
Unicode input, including the pathological cases the corpus does not happen
to contain (unpaired surrogates are not representable in Rust `&str`, but
adversarial byte sequences at UTF-8 boundaries, degenerate base64-alphabet
runs, deeply nested bidi-override sequences, and pathologically long
inputs are), never panics or hangs the char-by-char scanning loops in
`hidden_instructions.rs` (`invisible_unicode_char`,
`has_excessive_whitespace`, `find_base64_candidate`,
`has_hex_escape_sequence`, `decode_base64`) or the word-splitting matcher
in `scan/text_pattern.rs`.
