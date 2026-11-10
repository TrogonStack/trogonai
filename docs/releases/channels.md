# Release channels

Channels make the risk of an upgrade visible before you install it. A release
belongs to one channel at a time, and promotion should preserve the same
version identity across channels.

## Stable

Stable is the default channel for production workloads. It should contain a
version with published notes, a documented upgrade path, and the checks needed
for the supported deployment targets.

## Preview

Preview is for features that are complete enough to exercise but still need
feedback across more environments. Preview notes should call out known limits,
possible contract changes, and the point at which a feature is expected to move
to Stable.

## Nightly

Nightly tracks the latest automated build. It is useful for contributors and
integration environments that need early visibility, but it is not a support
or compatibility promise.

## Promotion checklist

Before moving a version to a broader channel, the release record should answer:

1. Which exact commit and artifacts does the version contain?
2. Which checks passed, and which remain unverified?
3. Are migrations, rollback steps, and compatibility changes documented?
4. Which deployment targets and runtime paths are covered?

Channel labels communicate intent. They do not replace the evidence recorded in
the release note.
