<div align="center">
  <img src="../brand/yolo@500x500.png" alt="Logo" width="300">
</div>

<div align="center">

# Experiments

</div>

This directory is the workspace for speculative code.

Normal repository taxonomy, naming, packaging, dependency, and polish rules are
relaxed here. It is acceptable for experiments to be messy, mixed-stack,
temporary, duplicated, or incomplete while an idea is being tested.

## License

This directory exists so ideas can be built in public while they are still
half-baked. Final licensing is deliberately deferred until an experiment
proves itself and graduates.

Until then, experiments default to the
[PolyForm Noncommercial License 1.0.0](./LICENSE) unless a more specific
license file, package manifest, source header, or written notice in the
relevant child path says otherwise. You are welcome to clone, run, modify,
and tinker with anything here for any noncommercial purpose.

The noncommercial default is not a statement of intent to stay closed. It is
a placeholder that keeps every option open: code can always be relicensed
permissively later, but a permissive grant cannot be taken back. Graduated
work has historically shipped as MIT or Apache 2.0, and the rest of this
repository is Apache 2.0. If you want to use an experiment commercially
before it graduates, open an issue and ask.

## Contributions

Contributions are welcome. By submitting a contribution (for example, a pull
request) to this directory, you agree to license your contribution to
Straw Hat, LLC under the Apache License 2.0, and you grant Straw Hat, LLC the
right to relicense and redistribute it, including as part of experiments that
graduate into the rest of this repository.

Do not assume anything under this directory is a package, workspace member,
published artifact, supported interface, runnable service, or maintained build
target. Files here only carry repository expectations where this README says
they still apply.

## Rules

- Production code must not depend on code in this directory.
- Experiments do not define stable APIs, SDKs, services, apps, or crates.
- Experiments may be deleted when they are stale or no longer useful.
- Secrets, credentials, private data, license obligations, and security
  expectations still apply.
- Anything that becomes real must graduate out of this directory before it is
  treated as part of the product or platform.

## Graduation

When an experiment becomes useful, move it into the normal workspace structure:

- Rust packages move into `rsworkspace/`.
- TypeScript or JavaScript packages move into `tsworkspace/`.
- Product-facing surfaces move into `apps/`.
- Production-operated workloads move into `services/`.
- Developer-facing callback/toolkit surfaces move into `sdks/`.
- Demonstrations move into `examples/`.

After graduation, the relevant ADRs and workspace rules apply again.
