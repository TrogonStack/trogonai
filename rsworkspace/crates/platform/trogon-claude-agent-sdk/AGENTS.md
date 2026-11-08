# Public API development

Keep review slices small. Add options and behavior incrementally.

- Keep SDK-owned configuration types named `*Options`.
- Validate independent inputs in value objects with private fields.
- Keep lifecycle operations explicit and preserve consuming `close(self)`.
- Unimplemented operations return a typed error; never report fake success.
- Introduce runtime effects only in a dedicated implementation slice.
- Add deterministic Python/TypeScript mappings and focused contract tests with
  each implemented capability; enforce compatibility checks in CI.
