# Publish and recover workload registration

Use this procedure to connect an independently protected approval controller
to the existing Unix registration runner. It covers the implemented producer
boundary. It does not supply the approval controller, provision an issuer or
establish deployment acceptance under draft
[ADR#0098](../adr/0098-attested-workload-identity.md).

For issuer and cluster preparation, use the
[deployment procedure](./deploy-workload-identity.md). For wire framing,
environment variables and terminal exit categories, use the
[runner reference](https://github.com/TrogonStack/trogonai/blob/main/rsworkspace/crates/platform/trogon-workload-registration/README.md).

## Establish independent owners

Assign each capability to its protected owner before starting the runner:

| Owner | Responsibility |
| --- | --- |
| Approver | Authorize exact finite snapshots and protect the Ed25519 approval signing key. |
| Provisioner | Create or verify the dedicated control/live streams and exact broker permissions. |
| Publisher | Verify approved frames and publish through one exclusive producer handle. It holds no approval signing key. |
| Emergency withdrawer | Submit an independently authorized whole-scope denial through the withdrawal command. |
| Recovery authority | Advance the independently managed fence, provision replacement state, revoke old writers and update readers. |
| Workload reader | Consume only its scoped authority and withdraw on loss or invalid state. It cannot publish its own renewal. |

Every raw writer credential for an authority stream is trusted control-plane
material. A subject ACL cannot inspect a protobuf body or turn a credential
into a cryptographically denial-only writer. Protect the runner, its controller
pipe and its broker credentials accordingly. Use distinct reply namespaces;
all untrusted roles sharing the account must lack broad `_INBOX.>` and `>`
subscriptions, or the roles must use separate accounts.

## Prepare the scope and protected streams

Select the exact tenant, project, trust domain, nonzero recovery fence and
approval public key. Obtain the fence from independently governed recovery
state, rather than restoring it with the stream data it protects.

Configure explicit authenticated TLS NATS access under the runner's environment
contract. With the separately constrained provisioning identity, invoke
`trogon-workload-registration provision` using those scope values. The runner
reference contains the complete invocation. Do not reuse the provisioner's
credential for the workload reader or the ordinary publisher.

The producer checks dedicated replicated file-backed control and live streams.
Each retains one complete snapshot on one exact scope subject. The live stream
expires its value; the control stream retains the claim and revision head.
Deletion, purge, extra subjects, source/mirror streams and incompatible stream
settings are refused. Provisioning a correct local profile does not prove
failover, backup or restore on the actual deployment.

## Supply fresh approved snapshots

The approver constructs the complete `RegistrationSnapshot` and uses the
producer's exported `approval_signing_bytes` function. It binds the operation
context, exact trust domain and canonical payload bytes. The runner accepts a
bounded binary frame through a readable Unix pipe or FIFO. It refuses JSON,
alternate protobuf encodings, regular-file stdin and unsupported hosts.

Start `serve` with the scoped publisher credential and the trusted controller's
pipe. The initial frame and subsequent partial frames have bounded input
budgets. Publication effects retain the original monotonic approval deadline;
a later wall-clock sample cannot restart that deadline.

For every new lease:

- Obtain a fresh independent signature. Possession of an earlier snapshot
  does not authorize renewal.
- Strictly advance the registration revision. Keep the trust revision only
  when the approved roots are unchanged.
- Preserve the exact tenant, project, trust domain and recovery fence.
- Keep the complete lease within the 330-second registration ceiling.

Changing bindings or approved roots requires an acknowledged whole-scope denial
before a replacement grant. Root changes also advance the applicable trust
revision. Empty bindings or empty approved roots represent valid complete
withdrawal while retaining revision floors.

The publisher waits for durable audit before publishing a grant. It captures
the live stream sequence and uses broker conflict fencing so a withdrawal
accepted during that audit wait can prevent the delayed grant. A conflicting,
cancelled or uncertain operation poisons the handle; it cannot continue issuing
updates as if nothing happened.

## Withdraw without waiting for later bookkeeping

Use `withdraw` with the independently constrained emergency identity and a
fresh signed denial frame. A grant is refused by this command. The operation
publishes complete denial to the live stream before later audit/control
bookkeeping. A later failure cannot restore an earlier grant.

A failed acknowledgement can leave the effect uncertain. Inspect the protected
live/control state and audit rather than assuming that a timeout meant nothing
was sent. The native SDK can retain queued work, so protected reconnection
quarantines replacement dials for longer than the maximum approval lifetime.
That control does not retract commands already dispatched, revoke old writer
credentials or replace independent restore fencing.

Consumer withdrawal, established-session closure and remote provider-token
revocation are different effects. Verify the selected runtime's physical close
path and record its evidence; successful publication alone does not prove it.

## Recover with a new fence

EOF, invalid input, approval expiry, interruption, timeout or uncertain effects
end the running producer. Do not configure a same-fence automatic restart.
An existing durable claim cannot be reclaimed merely because its live value
expired or a process disappeared.

Before enabling a replacement producer:

1. Deny affected use through the protected operational path and account for
   uncertain earlier publications.
2. Advance the recovery fence through its independent authority.
3. Provision and verify the new scope/fence streams.
4. Revoke the old publication credentials and reconcile old queued writers.
5. Update consumers to the new protected fence and stream identities.
6. Admit a newly reviewed snapshot only after its issuer, mapping and audit
   prerequisites are current.

Prove the sequence against actual backup/restore and writer-revocation behavior.
A counter restored with the same backup cannot establish that recovery occurred.
Clock integrity, controller custody, all-replica withdrawal and HA acceptance
remain deployment gates in the
[workload identity reference](../reference/workload-identity.md).
