# Prompt management: text and chat prompt shapes

Part of the agent instructions research corpus.
Evidence retrieved 2026-07-30 from official documentation. This document
answers a narrower question than the harness survey: when a product
manages prompt content as a first-class resource, what shapes does the
content take, and do those shapes appear in agent definitions?

## Langfuse

Langfuse prompt management types every managed prompt with
`type: text | chat`.

- A text prompt's content is a single string.
- A chat prompt's content is an array of role-tagged messages
  (`{role, content}` with system, user, and assistant roles), plus
  message placeholders for injecting chat history at runtime.
- The two are not interchangeable: the SDK enforces the expected type at
  fetch time, and the `type` field cannot be changed after creation.
- Versions form an immutable sequential history; labels (`production`,
  `latest`, custom) are movable pointers to versions. Shape changes ride
  new versions, never in-place mutation.

Anchor:
[Langfuse prompt data model](https://langfuse.com/docs/prompt-management/data-model).

## LangChain

The same split appears one level down, as classes. `PromptTemplate`
formats to a plain string for completion-style models and supports no
roles. `ChatPromptTemplate` formats to a list of system, human, and AI
messages, with `MessagesPlaceholder` for history. The two are distinct
types, not renderings of one another.

## Provider APIs

- Anthropic Messages API: `system` is `string | TextBlockParam[]`; the
  blocks exist for cache-control granularity, not for roles. Message
  `content` began as a string and became
  `string | ContentBlock[]`.
- OpenAI: message `content` likewise grew from string to
  string-or-content-parts. The dashboard-managed Prompt object
  (`{prompt_id, version, variables}`) is deprecated with `v1/prompts`
  shutting down 2026-11-30, and the Assistants API (string
  `instructions`) shuts down 2026-08-26; the migration guidance is to
  keep prompt text in versioned application code.

The `content` evolution is the ecosystem's canonical example of a string
field that later needed to be a list of typed parts. Providers handled it
as an accept-both union rather than a migration, and the union sits on
the message payload, not on any agent definition.

## The boundary this corpus draws

Chat-shaped prompt content exists where a product stores prompts for
arbitrary LLM call sites: full conversation scaffolds, few-shot
exchanges, history templating. That is prompt management.

Agent definitions across the corpus do not use it. Their instruction
fields are strings (OpenAI Assistants and Agents SDK, every harness in
the [harness survey](./harness-survey.md)), and where a union appears it
is string-or-preset or string-or-callable, never string-or-messages.

Two facts from Langfuse generalize beyond it:

1. Text and chat are alternative representations, not points on one
   gradient; a platform that supports both models them as a tagged
   union with an immutable tag.
2. Representation changes arrive as new immutable versions. An
   event-sourced revision model provides this property natively.
