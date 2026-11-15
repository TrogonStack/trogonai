# Standard Agents primary-source ledger

Companion evidence for the [dossier](../products/standard-agents.md) and
[platform comparison](../contract-impact-standard-agents.md).

## Retrieval and scope

- Retrieved: 2026-11-15, confirmed with the local clock.
- Repository snapshot: [`78fa54c14d7125f481f6f05538cce5abb5b7a419`](https://github.com/standardagents/spec/commit/78fa54c14d7125f481f6f05538cce5abb5b7a419).
- The repository README names 0.2.0 current. Versioned content, rather than a
  mutable `/latest` URL, supplies the behavioral claims.
- The commit permalink is the source snapshot. SHA-256 below hashes complete
  source-file bytes, including line endings; it does not claim an upstream
  release signature or certification.
- Inspected material includes the versioned specification and site metadata.
  Excerpts are literal substrings, with original Markdown retained. A short
  excerpt can be part of a longer source line; links identify that line.
- No SDK or runtime was installed or executed. The npm API examples are
  specification declarations, not independently verified implementation behavior.
- The GitHub releases API returned no release entries during retrieval. This
  observation does not establish the npm SDK version or its implementation.
- Open issues [#52](https://github.com/standardagents/spec/issues/52) and
  [#46](https://github.com/standardagents/spec/issues/46) were checked for topic
  and open status on the retrieval date; their discussions are not normative.

## Exact excerpts

### Agent definition

[content/0.2.0/specification/agents.md:3](https://github.com/standardagents/spec/blob/78fa54c14d7125f481f6f05538cce5abb5b7a419/content/0.2.0/specification/agents.md#L3):

> An agent is the top-level conversation orchestrator in Standard Agents.

[content/0.2.0/specification/agents.md:3](https://github.com/standardagents/spec/blob/78fa54c14d7125f481f6f05538cce5abb5b7a419/content/0.2.0/specification/agents.md#L3):

> There is no agent "type": everything about how an agent converses, composes, and terminates is **derived from the backings** of its two sides.

### Version boundary

[content/0.1.0/specification/agents.md:10](https://github.com/standardagents/spec/blob/78fa54c14d7125f481f6f05538cce5abb5b7a419/content/0.1.0/specification/agents.md#L10):

> A **subagent** is not a separate concept

[content/0.1.0/specification/agents.md:25](https://github.com/standardagents/spec/blob/78fa54c14d7125f481f6f05538cce5abb5b7a419/content/0.1.0/specification/agents.md#L25):

> | `type` | `'ai_human' \| 'dual_ai'` | `'ai_human'` | Conversation mode |

### Persistent children

[content/0.2.0/specification/subagents.md:118](https://github.com/standardagents/spec/blob/78fa54c14d7125f481f6f05538cce5abb5b7a419/content/0.2.0/specification/subagents.md#L118):

> **Every child instance is a persistent thread.**

[content/0.2.0/specification/subagents.md:110](https://github.com/standardagents/spec/blob/78fa54c14d7125f481f6f05538cce5abb5b7a419/content/0.2.0/specification/subagents.md#L110):

> When `blocking: true` is combined with `maxInstances: 1` and an instance already exists, `subagent_create` **MUST** behave as **create-or-continue**: message the existing instance and await *that* session's close, rather than erroring on the cap.

[content/0.2.0/specification/subagents.md:149](https://github.com/standardagents/spec/blob/78fa54c14d7125f481f6f05538cce5abb5b7a419/content/0.2.0/specification/subagents.md#L149):

> Child visibility is pull-based: runtimes inject the `subagents` query tool

### Handler delivery

[content/0.2.0/specification/handlers.md:61](https://github.com/standardagents/spec/blob/78fa54c14d7125f481f6f05538cce5abb5b7a419/content/0.2.0/specification/handlers.md#L61):

> - The runtime **MUST** invoke the handler exactly once for each inbound message on the side's perspective, in message persistence order.

[content/0.2.0/specification/handlers.md:63](https://github.com/standardagents/spec/blob/78fa54c14d7125f481f6f05538cce5abb5b7a419/content/0.2.0/specification/handlers.md#L63):

> - The runtime **MUST NOT** automatically redeliver a message whose handler invocation failed. Retry is a userland decision (re-send the message).

### Required parent hooks

[content/0.2.0/specification/agents.md:155](https://github.com/standardagents/spec/blob/78fa54c14d7125f481f6f05538cce5abb5b7a419/content/0.2.0/specification/agents.md#L155):

> Downloading or installing a package is inert.

[content/0.2.0/specification/agents.md:163](https://github.com/standardagents/spec/blob/78fa54c14d7125f481f6f05538cce5abb5b7a419/content/0.2.0/specification/agents.md#L163):

> - The hook receives the parent's `ThreadState` and remains active for the lifetime of the enabled relationship. It applies during every prompt on either side of the parent.

[content/0.2.0/specification/agents.md:164](https://github.com/standardagents/spec/blob/78fa54c14d7125f481f6f05538cce5abb5b7a419/content/0.2.0/specification/agents.md#L164):

> - Missing, invalid, or unloadable required hooks **MUST** fail composition closed. Runtimes must not silently run a partially installed child.

### Context assembly

[content/0.2.0/infrastructure/runtime.md:71](https://github.com/standardagents/spec/blob/78fa54c14d7125f481f6f05538cce5abb5b7a419/content/0.2.0/infrastructure/runtime.md#L71):

> The snapshot **MUST** be the set of eligible persisted messages at the moment the runtime starts `filter_messages`.

[content/0.2.0/infrastructure/runtime.md:88](https://github.com/standardagents/spec/blob/78fa54c14d7125f481f6f05538cce5abb5b7a419/content/0.2.0/infrastructure/runtime.md#L88):

> Tool calls **MUST** execute sequentially in the returned order.

### Silent history

[content/0.2.0/specification/threads/messages.md:128](https://github.com/standardagents/spec/blob/78fa54c14d7125f481f6f05538cce5abb5b7a419/content/0.2.0/specification/threads/messages.md#L128):

> - Silent messages **MUST** remain eligible for model context.

[content/0.2.0/specification/threads/messages.md:130](https://github.com/standardagents/spec/blob/78fa54c14d7125f481f6f05538cce5abb5b7a419/content/0.2.0/specification/threads/messages.md#L130):

> - `silent` **MUST NOT** change persistence, lifecycle hooks, or same-execution visibility.

### Decision inference

[content/0.2.0/specification/threads/decisions.md:3](https://github.com/standardagents/spec/blob/78fa54c14d7125f481f6f05538cce5abb5b7a419/content/0.2.0/specification/threads/decisions.md#L3):

> A decision evaluates named, typed questions against caller-supplied context.

[content/0.2.0/specification/threads/decisions.md:162](https://github.com/standardagents/spec/blob/78fa54c14d7125f481f6f05538cce5abb5b7a419/content/0.2.0/specification/threads/decisions.md#L162-L163):

> Rounded probabilities **MUST NOT** be silently
> renormalized.

[content/0.2.0/specification/threads/decisions.md:165](https://github.com/standardagents/spec/blob/78fa54c14d7125f481f6f05538cce5abb5b7a419/content/0.2.0/specification/threads/decisions.md#L165-L167):

> Providers and runtimes **MUST NOT** coerce a boolean probability to a boolean
> or apply an application threshold. Thresholds and subsequent actions are
> caller-defined.

### Tool dependency restriction

[content/0.2.0/specification/tools.md:44](https://github.com/standardagents/spec/blob/78fa54c14d7125f481f6f05538cce5abb5b7a419/content/0.2.0/specification/tools.md#L44):

> When a `uses` list is supplied, the runtime **MUST** reject an invocation of a target outside the list.

[content/0.2.0/specification/tools.md:44](https://github.com/standardagents/spec/blob/78fa54c14d7125f481f6f05538cce5abb5b7a419/content/0.2.0/specification/tools.md#L44):

> An empty list permits no targets. When `uses` is omitted, this restriction does not apply.

### Credential grants

[content/0.2.0/specification/users.md:199](https://github.com/standardagents/spec/blob/78fa54c14d7125f481f6f05538cce5abb5b7a419/content/0.2.0/specification/users.md#L199):

> 1. Secret material is write-only: it MUST NOT be readable by packaged code, models, or clients. Only `metadata` and grant existence are readable.

[content/0.2.0/specification/users.md:200](https://github.com/standardagents/spec/blob/78fa54c14d7125f481f6f05538cce5abb5b7a419/content/0.2.0/specification/users.md#L200):

> 2. `fetch(grant, input, init)` MUST inject the credential per the grant's `kind` and `inject` policy on the runtime side.

[content/0.2.0/specification/users.md:202](https://github.com/standardagents/spec/blob/78fa54c14d7125f481f6f05538cce5abb5b7a419/content/0.2.0/specification/users.md#L202):

> 4. The runtime MUST reject a `fetch` from a caller not covered by `allow`. An absent `allow` denies all callers.

[content/0.2.0/specification/users.md:204](https://github.com/standardagents/spec/blob/78fa54c14d7125f481f6f05538cce5abb5b7a419/content/0.2.0/specification/users.md#L204):

> 6. Deleting a grant MUST cause subsequent `fetch` calls to fail with an error distinguishable from a network failure.

### Model fallback

[content/0.2.0/specification/models.md:44](https://github.com/standardagents/spec/blob/78fa54c14d7125f481f6f05538cce5abb5b7a419/content/0.2.0/specification/models.md#L44):

> When a model definition includes `fallbacks`, implementations **SHOULD**:

[content/0.2.0/specification/models.md:47](https://github.com/standardagents/spec/blob/78fa54c14d7125f481f6f05538cce5abb5b7a419/content/0.2.0/specification/models.md#L47):

> 2. If the primary model fails after exhausting retries, try each fallback in order

### Client API scope

[content/0.2.0/specification/threads/client-api.md:9](https://github.com/standardagents/spec/blob/78fa54c14d7125f481f6f05538cce5abb5b7a419/content/0.2.0/specification/threads/client-api.md#L9):

> 1. A runtime **SHOULD** expose the thread client API.

[content/0.2.0/specification/threads/client-api.md:10](https://github.com/standardagents/spec/blob/78fa54c14d7125f481f6f05538cce5abb5b7a419/content/0.2.0/specification/threads/client-api.md#L10):

> 2. A runtime that exposes it **MUST** implement every route in §2 as specified in this section.

[content/0.2.0/specification/threads/client-api.md:44](https://github.com/standardagents/spec/blob/78fa54c14d7125f481f6f05538cce5abb5b7a419/content/0.2.0/specification/threads/client-api.md#L44):

> 4. The runtime **MUST NOT** accept a long-lived credential in a URL.

### Packaging portability

[content/0.2.0/distribution/packaging.md:10](https://github.com/standardagents/spec/blob/78fa54c14d7125f481f6f05538cce5abb5b7a419/content/0.2.0/distribution/packaging.md#L10-L13):

> Packed definitions MUST NOT require private runtime
> RPCs, implementation-specific storage schemas or bindings, undocumented
> events, sidecar services that substitute for runtime behavior, or
> `_notPackableRuntimeContext`.

### Runtime directory limits

[src/data/runtimes.md:24](https://github.com/standardagents/spec/blob/78fa54c14d7125f481f6f05538cce5abb5b7a419/src/data/runtimes.md#L24):

> A listing is not an endorsement, and it does not certify conformance.

## Source-file digests

Paths are relative to `standardagents/spec` at the pinned commit.

| Source | SHA-256 |
| --- | --- |
| [content/0.2.0/distribution/packaging.md](https://github.com/standardagents/spec/blob/78fa54c14d7125f481f6f05538cce5abb5b7a419/content/0.2.0/distribution/packaging.md) | `5f9180c43b7c5a43f8bad3925ae6c729bb2777579f2d63035e96a6ff2a011bb1` |
| [content/0.2.0/infrastructure/providers.md](https://github.com/standardagents/spec/blob/78fa54c14d7125f481f6f05538cce5abb5b7a419/content/0.2.0/infrastructure/providers.md) | `bb0c357f2f85b4860cc3be7db7f5207cc46bfa2320881c632320cb9692de2fcb` |
| [content/0.2.0/infrastructure/runtime.md](https://github.com/standardagents/spec/blob/78fa54c14d7125f481f6f05538cce5abb5b7a419/content/0.2.0/infrastructure/runtime.md) | `7893ce53d71ed23d5ac675293123aa3e8cbdfe9a000acfd0168def2b3f1f3693` |
| [content/0.2.0/specification/agents.md](https://github.com/standardagents/spec/blob/78fa54c14d7125f481f6f05538cce5abb5b7a419/content/0.2.0/specification/agents.md) | `05fc056303eab738065efa51f0d9b8dc11909322862ec786f0bbf8a416646a31` |
| [content/0.2.0/specification/effects.md](https://github.com/standardagents/spec/blob/78fa54c14d7125f481f6f05538cce5abb5b7a419/content/0.2.0/specification/effects.md) | `ebb11b27fd8af04f6fe6a5d00ae48c2e1a5b85d320236cc13acd061391f8f76d` |
| [content/0.2.0/specification/handlers.md](https://github.com/standardagents/spec/blob/78fa54c14d7125f481f6f05538cce5abb5b7a419/content/0.2.0/specification/handlers.md) | `3493d5297297cc94e017a1833c3daa2cce19face3f3eb486baa0d18f2c9fc0fa` |
| [content/0.2.0/specification/hooks.md](https://github.com/standardagents/spec/blob/78fa54c14d7125f481f6f05538cce5abb5b7a419/content/0.2.0/specification/hooks.md) | `659d3b98c6e58797b97816c159cf0a345346b63f6c7915028336bc98978ccfef` |
| [content/0.2.0/specification/models.md](https://github.com/standardagents/spec/blob/78fa54c14d7125f481f6f05538cce5abb5b7a419/content/0.2.0/specification/models.md) | `5eb29e1e740fee0a9ba8cac02d5f6c190483241081820fb86f788f33561d0282` |
| [content/0.2.0/specification/prompts.md](https://github.com/standardagents/spec/blob/78fa54c14d7125f481f6f05538cce5abb5b7a419/content/0.2.0/specification/prompts.md) | `91062950c17ade55a248164b21b2835ef0e7c72280c123a3f3a91f6f828c7f9f` |
| [content/0.2.0/specification/providers.md](https://github.com/standardagents/spec/blob/78fa54c14d7125f481f6f05538cce5abb5b7a419/content/0.2.0/specification/providers.md) | `e80825414a9d667e855e292bf6e4f83515a4e072c2c2f0ad83b9cd7a2f0928fd` |
| [content/0.2.0/specification/subagents.md](https://github.com/standardagents/spec/blob/78fa54c14d7125f481f6f05538cce5abb5b7a419/content/0.2.0/specification/subagents.md) | `f71ca8dcfbe230e97b1776ff8cd5844c197088b5cebb642197a33d15858ceaaa` |
| [content/0.2.0/specification/threads/client-api.md](https://github.com/standardagents/spec/blob/78fa54c14d7125f481f6f05538cce5abb5b7a419/content/0.2.0/specification/threads/client-api.md) | `e1147f5b9958e6c45e465ef7ea754349df9da786f6664dcc942307f5281ca564` |
| [content/0.2.0/specification/threads/code-execution.md](https://github.com/standardagents/spec/blob/78fa54c14d7125f481f6f05538cce5abb5b7a419/content/0.2.0/specification/threads/code-execution.md) | `5ba6d186049359b0f885e9442a16e538e6c731df68a5c9dab014725403194f78` |
| [content/0.2.0/specification/threads/decisions.md](https://github.com/standardagents/spec/blob/78fa54c14d7125f481f6f05538cce5abb5b7a419/content/0.2.0/specification/threads/decisions.md) | `15c646172799c052ffdb559d7fad1512ba0ff3a6cb955116835d331e14cb6dbd` |
| [content/0.2.0/specification/threads/endpoints.md](https://github.com/standardagents/spec/blob/78fa54c14d7125f481f6f05538cce5abb5b7a419/content/0.2.0/specification/threads/endpoints.md) | `e267aa40a81cb1bb0e00a04fb591326baffa5c42f954196a09ad1d8db5060825` |
| [content/0.2.0/specification/threads/execution-state.md](https://github.com/standardagents/spec/blob/78fa54c14d7125f481f6f05538cce5abb5b7a419/content/0.2.0/specification/threads/execution-state.md) | `e12fd7942e55707cdc3b894b99b5cff7e572e4674c445b4fe728e78829a7bb12` |
| [content/0.2.0/specification/threads/filesystem.md](https://github.com/standardagents/spec/blob/78fa54c14d7125f481f6f05538cce5abb5b7a419/content/0.2.0/specification/threads/filesystem.md) | `455df74632b8fea9cada7bf270c5a265811f450691fd7424c08742f1a5332fbe` |
| [content/0.2.0/specification/threads/messages.md](https://github.com/standardagents/spec/blob/78fa54c14d7125f481f6f05538cce5abb5b7a419/content/0.2.0/specification/threads/messages.md) | `1ba5d7143a192e08413679b274b9b1181f1c9698896b244b2ac5b8d8e3877800` |
| [content/0.2.0/specification/threads/runtime-streams.md](https://github.com/standardagents/spec/blob/78fa54c14d7125f481f6f05538cce5abb5b7a419/content/0.2.0/specification/threads/runtime-streams.md) | `659ca050f508bbe7e3422d5ea16f5fed5ed164d1c9e108823b8d1a569d3f55df` |
| [content/0.2.0/specification/threads.md](https://github.com/standardagents/spec/blob/78fa54c14d7125f481f6f05538cce5abb5b7a419/content/0.2.0/specification/threads.md) | `33b7d1eed207f78c551c17aea09fa7525008e2b7218d38d6c0e47777c46d32e4` |
| [content/0.2.0/specification/tools.md](https://github.com/standardagents/spec/blob/78fa54c14d7125f481f6f05538cce5abb5b7a419/content/0.2.0/specification/tools.md) | `2439c24fcd43d666ce7d2df5897aed431bb684ffe91e5a2bab7fda29852ac5f6` |
| [content/0.2.0/specification/type-registry.md](https://github.com/standardagents/spec/blob/78fa54c14d7125f481f6f05538cce5abb5b7a419/content/0.2.0/specification/type-registry.md) | `def64cb9772816ceba40568d27afe1c6e5c0849a96a9ee61fcbd8634c160d53e` |
| [content/0.2.0/specification/users.md](https://github.com/standardagents/spec/blob/78fa54c14d7125f481f6f05538cce5abb5b7a419/content/0.2.0/specification/users.md) | `bd130a9716df404deb2cedd26454c3f60844f91e0315e753f335f883dcad78ee` |
| [README.md](https://github.com/standardagents/spec/blob/78fa54c14d7125f481f6f05538cce5abb5b7a419/README.md) | `17823b944bbb2a814ec304b75c8c6a5b7f4917bba76836785ead506e7d2b5966` |
| [package.json](https://github.com/standardagents/spec/blob/78fa54c14d7125f481f6f05538cce5abb5b7a419/package.json) | `106955bc8c8678f68bb2e71668a821135f93b149a568db4fe357ad3ee35c950d` |
| [content/0.1.0/specification/agents.md](https://github.com/standardagents/spec/blob/78fa54c14d7125f481f6f05538cce5abb5b7a419/content/0.1.0/specification/agents.md) | `8c1991cdddeeaba7522de5ce0deb0e5b44c1834a72ffbd2a3163e7838e3027be` |
| [src/data/runtimes.md](https://github.com/standardagents/spec/blob/78fa54c14d7125f481f6f05538cce5abb5b7a419/src/data/runtimes.md) | `085f518f2ff052797f5695827138fff23fe21426556ae5451a9eaab73d067976` |
| [tests/site.test.mjs](https://github.com/standardagents/spec/blob/78fa54c14d7125f481f6f05538cce5abb5b7a419/tests/site.test.mjs) | `4ce7db7c04a918cfceceddb6ff3b02d5f7150707c949967d55903568b5396225` |
| [tests/models-guide.test.mjs](https://github.com/standardagents/spec/blob/78fa54c14d7125f481f6f05538cce5abb5b7a419/tests/models-guide.test.mjs) | `9601ce2a141162ceebf72a8fe6326b7f252bbd7a04e2c367d8acbe0db4c737ed` |
| [tests/home-cta.test.mjs](https://github.com/standardagents/spec/blob/78fa54c14d7125f481f6f05538cce5abb5b7a419/tests/home-cta.test.mjs) | `f5b84dc5425e923dfce0fbf794d4f6158723f03dbd9a7b69079ea836525fbf57` |

## Local comparison anchors

The comparison baseline is `3d5354507dd6d937ea70eaa957e7f31332d82dfb`.
These repository paths and symbols were checked directly. Draft schemas are
declarations; implementation files are static evidence, not fresh test receipts.

| Source | Relevant symbol or boundary |
| --- | --- |
| [buf.gen.yaml:13](https://github.com/TrogonStack/trogonai/blob/3d5354507dd6d937ea70eaa957e7f31332d82dfb/buf.gen.yaml#L13) | Agent, Session and adjacent package generation exclusions |
| [proto/trogonai/agents/agents/v1alpha1/agent.proto:18](https://github.com/TrogonStack/trogonai/blob/3d5354507dd6d937ea70eaa957e7f31332d82dfb/proto/trogonai/agents/agents/v1alpha1/agent.proto#L18) | Runtime settings, dependencies and execution subject |
| [proto/trogonai/agents/agents/v1alpha1/agent_revision.proto:13](https://github.com/TrogonStack/trogonai/blob/3d5354507dd6d937ea70eaa957e7f31332d82dfb/proto/trogonai/agents/agents/v1alpha1/agent_revision.proto#L13) | Exact configuration reference and revision identity |
| [proto/trogonai/agents/agents/v1alpha1/implementation.proto:89](https://github.com/TrogonStack/trogonai/blob/3d5354507dd6d937ea70eaa957e7f31332d82dfb/proto/trogonai/agents/agents/v1alpha1/implementation.proto#L89) | Pinned implementation and definition digest |
| [proto/trogonai/agents/agents/v1alpha1/dependencies.proto:19](https://github.com/TrogonStack/trogonai/blob/3d5354507dd6d937ea70eaa957e7f31332d82dfb/proto/trogonai/agents/agents/v1alpha1/dependencies.proto#L19) | Distinct declared binding names and dependency resolution |
| [proto/trogonai/session/sessions/v1alpha1/session_started.proto:18](https://github.com/TrogonStack/trogonai/blob/3d5354507dd6d937ea70eaa957e7f31332d82dfb/proto/trogonai/session/sessions/v1alpha1/session_started.proto#L18) | Immutable plan creation fact |
| [proto/trogonai/session/sessions/v1alpha1/execution_plan.proto:11](https://github.com/TrogonStack/trogonai/blob/3d5354507dd6d937ea70eaa957e7f31332d82dfb/proto/trogonai/session/sessions/v1alpha1/execution_plan.proto#L11) | Canonical plan bytes and digest |
| [proto/trogonai/session/sessions/v1alpha1/close_session.proto:12](https://github.com/TrogonStack/trogonai/blob/3d5354507dd6d937ea70eaa957e7f31332d82dfb/proto/trogonai/session/sessions/v1alpha1/close_session.proto#L12) | Terminal Session boundary |
| [proto/trogonai/session/sessions/v1alpha1/create_child_session.proto:22](https://github.com/TrogonStack/trogonai/blob/3d5354507dd6d937ea70eaa957e7f31332d82dfb/proto/trogonai/session/sessions/v1alpha1/create_child_session.proto#L22) | Proposed child lineage saga and live admission |
| [proto/trogonai/session/sessions/v1alpha1/reserve_operation.proto:15](https://github.com/TrogonStack/trogonai/blob/3d5354507dd6d937ea70eaa957e7f31332d82dfb/proto/trogonai/session/sessions/v1alpha1/reserve_operation.proto#L15) | Operation identity and request digest reservation |
| [proto/trogonai/session/sessions/v1alpha1/cascade_policy.proto:8](https://github.com/TrogonStack/trogonai/blob/3d5354507dd6d937ea70eaa957e7f31332d82dfb/proto/trogonai/session/sessions/v1alpha1/cascade_policy.proto#L8) | Explicit parent-terminal cascade versus independent child |
| [rsworkspace/crates/decider/trogon-decider-nats/src/lib.rs:1](https://github.com/TrogonStack/trogonai/blob/3d5354507dd6d937ea70eaa957e7f31332d82dfb/rsworkspace/crates/decider/trogon-decider-nats/src/lib.rs#L1) | Native JetStream storage, processor and projector modules |
| [rsworkspace/crates/scheduler/trogon-scheduler/src/processor/execution/worker/mod.rs:1](https://github.com/TrogonStack/trogonai/blob/3d5354507dd6d937ea70eaa957e7f31332d82dfb/rsworkspace/crates/scheduler/trogon-scheduler/src/processor/execution/worker/mod.rs#L1) | Existing scheduling dispatcher, retry and reconciliation exports |
| [rsworkspace/crates/policy/trogon-policy-eval/src/evaluator.rs:32](https://github.com/TrogonStack/trogonai/blob/3d5354507dd6d937ea70eaa957e7f31332d82dfb/rsworkspace/crates/policy/trogon-policy-eval/src/evaluator.rs#L32) | Pure policy evaluation over snapshots |
| [rsworkspace/crates/platform/trogon-egress-proxy/src/lib.rs:1](https://github.com/TrogonStack/trogonai/blob/3d5354507dd6d937ea70eaa957e7f31332d82dfb/rsworkspace/crates/platform/trogon-egress-proxy/src/lib.rs#L1) | CONNECT policy/audit scope and implementation modules |
| [rsworkspace/crates/platform/trogon-workload-identity/README.md:1](https://github.com/TrogonStack/trogonai/blob/3d5354507dd6d937ea70eaa957e7f31332d82dfb/rsworkspace/crates/platform/trogon-workload-identity/README.md#L1) | Governed identity, current authority and caller obligations |
| [rsworkspace/crates/provider/trogon-provider-gateway/README.md:1](https://github.com/TrogonStack/trogonai/blob/3d5354507dd6d937ea70eaa957e7f31332d82dfb/rsworkspace/crates/provider/trogon-provider-gateway/README.md#L1) | Current HTTP foundation and unimplemented inference routes |
| [rsworkspace/crates/platform/trogon-claude-agent-sdk/README.md:1](https://github.com/TrogonStack/trogonai/blob/3d5354507dd6d937ea70eaa957e7f31332d82dfb/rsworkspace/crates/platform/trogon-claude-agent-sdk/README.md#L1) | Concrete Claude Code loop ownership and protocol boundary |
| [docs/architecture/agent-platform.md:1](https://github.com/TrogonStack/trogonai/blob/3d5354507dd6d937ea70eaa957e7f31332d82dfb/docs/architecture/agent-platform.md#L1) | Current foundations versus target architecture |

## Interpretation limits

Capitalized MUST, SHOULD and MAY retain the levels stated in the source.
Optional capability support is distinct from mandatory behavior once a capability
is implemented. Inconsistencies listed in the dossier remain open; this research
does not silently resolve them or claim upstream conformance.
