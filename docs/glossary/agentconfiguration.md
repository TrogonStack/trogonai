---
term: "AgentConfiguration"
section: "Agent execution model"
order: 1
---

# AgentConfiguration

The immutable configuration bound to an AgentRevision: one exact runtime
binding, one runtime-owned typed settings message carried in
`google.protobuf.Any`, and revision-owned platform declarations. The runtime
defines its native fields and validation. The platform defines common resource
declarations, such as skill pins and memory dependencies, whose use requires
support from the selected runtime or adapter. The configuration digest commits
to both. See draft
[ADR#0062](../adr/0062-runtime-owned-settings-and-platform-declarations.md).
