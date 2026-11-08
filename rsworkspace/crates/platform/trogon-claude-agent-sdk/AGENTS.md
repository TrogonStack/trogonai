# Public API development

Keep review slices small. Add options and behavior incrementally.

- Keep SDK-owned configuration types named `*Options`.
- Follow upstream terminology and field meanings; do not invent local aliases.
- Use Python's interactive client names, including `connect` and `disconnect`.
  Document deliberate Rust ownership differences such as the async factory and
  consuming `disconnect(self)`, and identify TypeScript counterparts separately.
- Add value-object restrictions only when grounded in an upstream invariant.
  Keep model strings and optional paths unrestricted at configuration assembly;
  process-specific validation belongs at the later process-start boundary.
- Unimplemented operations return a typed error; never report fake success.
- Introduce runtime effects only in a dedicated implementation slice.
- Add deterministic Python/TypeScript mappings and focused contract tests with
  each implemented capability; enforce compatibility checks in CI.
