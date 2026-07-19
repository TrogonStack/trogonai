# Event Metadata

Event payloads are the canonical domain facts used for replay, projections, and
business behavior. Event headers are envelope metadata for operational context
such as correlation, [tenancy](../glossary/tenant), causation, or [transport](../glossary/transport) routing.
Recorder-assigned time belongs to the persisted event envelope as `recorded_at`,
not to individual domain payloads.

The [decider](../glossary/decider) runtime should not derive required headers from [commands](../glossary/command) or emitted
events through a generic callback. If a workflow requires a fixed header set,
make that requirement explicit before command execution with a typed input owned
by the application boundary, validate it there, then pass the validated
`Headers` into `CommandExecution::with_headers`.

This keeps metadata policy close to the caller that knows why the metadata is
mandatory, while the runtime stays responsible for execution and storage
adapters stay responsible for persisting event envelopes.
