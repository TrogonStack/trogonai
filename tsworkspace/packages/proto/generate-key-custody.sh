#!/usr/bin/env bash
# Generates the TypeScript key custody bindings from a clean output set, with
# the buf executable given as the first argument.
set -euo pipefail

buf="${1:-buf}"
cd "$(dirname "$0")"
output=src/gen

if [[ -d "$output/trogonai/platform/key_custody" ]]; then
  find "$output/trogonai/platform/key_custody" -type f -delete
fi

for imported_binding in \
  "$output/buf/validate/validate_pb.ts" \
  "$output/elixirpb_pb.ts" \
  "$output/trogon/error/v1alpha1/code_pb.ts" \
  "$output/trogon/error/v1alpha1/options_pb.ts" \
  "$output/trogon/error/v1alpha1/visibility_pb.ts" \
  "$output/trogon/hierarchy/v1alpha1/node_pb.ts"
do
  if [[ -f "$imported_binding" ]]; then
    unlink "$imported_binding"
  fi
done

exec "$buf" generate --template buf.key-custody.gen.yaml
