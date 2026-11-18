# Contributing

## Development

Use the [Bazel build guide](./docs/how-to/use-bazel.md) for the native target
slice, dependency graph inspection, and GitHub Actions cache behavior.

## Continuous integration

Bazel runs the build, lint, and test checks; the `Bazel result` check must pass
before a pull request merges. Run `mise run --skip-tools bazel:test` to reproduce
it locally. The few checks that cannot run in Bazel each keep a small workflow,
listed in [What runs outside Bazel](./docs/how-to/use-bazel.md#what-runs-outside-bazel).
