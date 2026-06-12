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

By default, experiments are UNLICENSED and proprietary to Straw Hat, LLC unless
a more specific license file, package manifest, source header, or written notice
in the relevant child path says otherwise.

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
