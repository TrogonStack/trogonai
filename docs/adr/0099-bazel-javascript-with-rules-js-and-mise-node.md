---
number: "0099"
slug: bazel-javascript-with-rules-js-and-mise-node
status: draft
date: 2026-11-13
---

# ADR#0099: Bazel Builds and Tests the JavaScript and TypeScript Projects with rules_js, on the Node.js mise Installs

## Context

Bazel already builds the Go and Rust workspaces. Both use toolchains that mise
installed, through the `mise-bazel` module in `projects/mise-bazel`: mise owns
installation and its lockfile, and Bazel links the installed files into an
external repository and hashes them as build inputs.

The JavaScript and TypeScript projects were outside that graph. The
documentation site in `docs/` and the pnpm workspace in `tsworkspace/` (the
console app and the `@trogonai/proto` and `slack-bolt-fixtures` packages) were
built, typechecked, linted and tested only through pnpm scripts. The tests of
the GitHub Actions in `.github/actions` ran only through `node --test`. Nothing
connected a change to a Markdown page, a protobuf binding or a shared action to
the targets it affects, and the Bazel CI job could not see them at all.

The JavaScript projects use two separate pnpm lockfiles, pin Node.js and pnpm
through mise like every other tool, and run on pnpm 11, which reads lifecycle
script permissions from `allowBuilds` in `pnpm-workspace.yaml`.

## Decision

### Node.js comes from mise, never from a download

`mise-bazel` gains a `mise_node` repository for the `node` tool. It links the
mise install and declares a `nodejs_toolchain` from `rules_nodejs` with two
`toolchain` targets, one for the build tool type and one for the runtime type.
The root module registers both. Root registrations take precedence over the
default toolchains that `rules_nodejs` registers for itself, so Bazel resolves
Node.js to the mise install and never fetches one.

The toolchain covers macOS and Linux hosts on ARM64 and x86-64. Any other host
fails with an explicit message instead of resolving to a downloaded Node.js.

### rules_js and rules_ts, at versions tested on Bazel 9

The projects build with `aspect_rules_js` 3.6.1 and `aspect_rules_ts` 3.10.1,
the newest releases in the Bazel Central Registry. Both run their presubmit on
Bazel 9, which this repository uses, and `rules_js` declares Bazel 7.6 as its
floor. `rules_nodejs` 6.7.7 comes in through `mise-bazel` for the toolchain
definitions.

rules_ts runs the TypeScript compiler version that the console manifest pins,
with the integrity taken from the pnpm lockfile, so Bazel and pnpm typecheck
with the same compiler.

### pnpm owns the lockfiles, and each lockfile is its own repository

Each pnpm lockfile has one `npm_translate_lock`, with its own repository name:
`npm_docs` for `docs/pnpm-lock.yaml` and `npm_tsworkspace` for
`tsworkspace/pnpm-lock.yaml`. Every package that has dependencies links them
with `npm_link_all_packages`. `update_pnpm_lock` is off, so Bazel reads the
lockfiles and never rewrites them. Developers keep using pnpm to change
dependencies, and the lockfile pnpm writes is the one Bazel consumes.

The `node_modules` directories of every pnpm package are in `.bazelignore`, and
`npm_translate_lock` verifies that, so a local `pnpm install` cannot leak into
the build.

### Lifecycle scripts are allowed explicitly, with pnpm 11's setting

rules_js reads the same setting as pnpm 11, `allowBuilds` in
`pnpm-workspace.yaml`, to decide which packages run their install scripts.
`docs/` already allows `esbuild`, so its postinstall runs as a Bazel action.
The tsworkspace lockfile has no package that needs one, so its workspace file
declares an empty `allowBuilds`. That records the decision for pnpm and tells
Bazel that no lifecycle actions are needed, instead of leaving rules_js without
an answer.

### pnpm's hidden hoisting is replaced where a package relies on it

rules_js lays out packages like pnpm but does not reproduce pnpm's hidden
hoisting. Two console dependencies rely on it. `@testing-library/jest-dom`
augments and imports `vitest` without declaring it as a dependency, and the
TanStack declarations import `react`, whose types live in `@types/react`, which
they do not declare either. `npm_tsworkspace` hoists `vitest` and
`@types/react` to `tsworkspace/node_modules` with `public_hoist_packages`, and
the console targets depend on those links explicitly. Without them the
jest-dom matchers lose their types and the TanStack form render props degrade
to implicit `any`, and the typecheck fails.

### The targets

- `//docs:site` runs `vitepress build` over every file of the site and outputs
  the built site as one tree artifact. Its inputs are a glob of the package,
  so editing any page invalidates the site and unrelated changes do not.
- `//tsworkspace/packages/proto:pkg` is an `npm_package`, and the console links
  it as `@trogonai/proto` through the workspace dependency in its manifest, so
  console targets rebuild when the bindings change.
- `//tsworkspace/apps/console:routes` runs the TanStack Router CLI and declares
  `src/routeTree.gen.ts` as its output. The file is gitignored, so the build
  produces it instead of relying on a developer having run the generator.
- Each tsworkspace package has a `typecheck` `ts_project` with `no_emit`.
  Vite and vitest consume the TypeScript sources directly, so the compiler only
  checks.
- `//tsworkspace/apps/console:build` runs `vite build` and outputs `dist`.
  Rolldown resolves `index.html` through its sandbox symlink to the real
  execroot path and then rejects the asset name that path produces, so this one
  action runs without the sandbox. Its inputs are still declared.
- The console and `slack-bolt-fixtures` run vitest as Bazel tests, and each
  package runs oxlint as a test with its own configuration.
- `//tests/github_actions:runs_on_fallback_test` runs the `runs-on-fallback`
  action test against the action sources, which `//.github:actions` copies to
  the output tree.

### BUILD files are written by hand, not generated by Gazelle

The JavaScript Gazelle extension was evaluated and not adopted.
`aspect_gazelle_js` builds its parser from Rust source, registers a Rust
toolchain that it downloads, and registers a hermetic LLVM C++ toolchain
outside of development dependencies. Both contradict the rule that toolchains
come from mise, and the LLVM registration would compete with the C++ toolchain
resolution that the Rust and Go builds rely on. `aspect_gazelle_prebuilt`
instead downloads prebuilt binaries from GitHub releases, which is the same
download problem in another form. The repository does not generate Go BUILD
files with Gazelle either. Gazelle only resolves `go_deps`. The JavaScript
packages are few and change shape rarely, so hand-written BUILD files are
cheaper than a second generator.

### What stays outside Bazel

- **The CI selection tests** in `tests/github_actions`
  (`test_ci_selection.mjs`, `test_ci_selector_process.mjs`) and the Python
  canary test execute the mise task scripts under `.config/mise/tasks`. That
  directory is in `.bazelignore`, because task scripts named `build` collide
  with `BUILD` on case-insensitive filesystems, so Bazel cannot see the code
  under test. The selector tests also resolve their dependencies from an npm
  `package-lock.json` rather than a pnpm lockfile. They keep running in their
  own workflow.
- **The Claude SDK maintenance helpers** in
  `rsworkspace/crates/platform/trogon-claude-agent-sdk/.config/mise/lib`
  (`claude-sdk-compatibility` and `claude-sdk-freshness`) stay mise tasks. The
  freshness check downloads the newest SDK releases from PyPI and the npm
  registry on every run, which is the opposite of a cacheable action. The
  compatibility readers are maintenance tooling driven by mise tasks, mostly in
  Python, each with its own lockfile, and do not gate any build.
- **The pnpm based documentation workflows** (`ci-docs` and `docs-pages`) are
  unchanged. Moving them onto `//docs:site` is separate work.

### CI

The Bazel workflow installs `node` through mise alongside the other toolchains,
and runs when `docs/`, `tsworkspace/`, `tests/github_actions/` or
`.github/actions/` change. Its build and test tasks already target `//...`, so
they cover the new targets without listing them.

## Consequences

- A change to a documentation page, a protobuf binding or a shared action
  rebuilds and retests exactly what depends on it, in the same job and with the
  same caches as the Go and Rust targets.
- Bazel and pnpm read the same lockfiles and the same `allowBuilds`, so neither
  can drift from the other. Adding a dependency is still a pnpm operation,
  followed by a BUILD file edit when a target needs the new package.
- Adding a package with an install script means adding it to `allowBuilds`,
  which pnpm and Bazel both honor.
- A package that relies on pnpm's hidden hoisting fails under Bazel until it is
  hoisted explicitly. That surfaces undeclared dependencies that pnpm hides.
- The console production build is the one action that runs without the
  sandbox, so an undeclared input there is not caught by sandboxing.
- New JavaScript packages need hand-written BUILD files.

## References

- [aspect_rules_js](https://github.com/aspect-build/rules_js)
- [aspect_rules_ts](https://github.com/aspect-build/rules_ts)
- [rules_nodejs](https://github.com/bazel-contrib/rules_nodejs)
- [pnpm `allowBuilds`](https://pnpm.io/settings#allowbuilds)
- `projects/mise-bazel/README.md`
