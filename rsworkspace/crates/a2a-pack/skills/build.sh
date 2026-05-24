#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../../.." && pwd)"
TARGET="${CARGO_TARGET_DIR:-$ROOT/target}/wasm32-unknown-unknown/release"
WRAP="$ROOT/debug/wrap-redact-export"
SKILLS_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

cd "$ROOT"
cargo build -p tier3-skill-abi --bin wrap-redact-export
cargo build -p pii-regex-redactor -p secrets-redactor -p json-path-sanitizer --target wasm32-unknown-unknown --release

"$WRAP" "$TARGET/pii_regex_redactor.wasm" "$SKILLS_DIR/pii_regex_redactor.wasm"
"$WRAP" "$TARGET/secrets_redactor.wasm" "$SKILLS_DIR/secrets_redactor.wasm"
"$WRAP" "$TARGET/json_path_sanitizer.wasm" "$SKILLS_DIR/json_path_sanitizer.wasm"

echo "tier-3 skills rebuilt under $SKILLS_DIR"
