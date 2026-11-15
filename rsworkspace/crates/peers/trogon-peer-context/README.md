# Peer context rendering

`PeerContext::render(&batch, &messages)` converts generated peer delivery
messages into bounded text for native host context. The result contains a fixed
untrusted-data notice followed by JSON with batch identity, message identity,
sender, recipient, collaboration, acceptance time, and text. `as_str()` exposes
that text; `Debug` reports only its byte count.

A complete assignment contains at most four messages, each at most 2 KiB of
UTF-8 text. The entire rendered result, including the notice, provenance, JSON,
and escaping, is at most 8 KiB. The renderer rejects malformed, incomplete,
duplicate, reordered, or oversized assignments with `ContractViolation`. It
never truncates text or removes messages from a recorded assignment. Valid
protobuf timestamps, canonical peer identities, and exact declared text byte
counts are required.

The trusted service can use the same renderer while selecting a candidate batch
before persisting its assignment. The host adapter uses it again on the assigned
batch. Raw body sizes alone do not establish the rendered limit: escaping can
make the presentation substantially larger.

JSON control characters and host-significant markup characters are escaped as
data. The notice preserves existing instructions and approval requirements and
asks for `peer_acknowledge_batch` after considering the messages. That tool
records application acknowledgment; it supplies no proof of host-context
insertion, model understanding, or task completion. This presentation does not
eliminate prompt injection.

The renderer checks the assignment's shape and internal consistency. It does not
authenticate a sender, authorize a content read, verify a body digest, or check
that the recipient and collaboration equal the current host enrollment. Those
checks belong to the trusted service and bound adapter. They must occur before
the returned text reaches the host. No message body or content reference is
stored by this crate.
