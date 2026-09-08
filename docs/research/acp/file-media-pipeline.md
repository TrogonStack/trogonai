# File and Media Pipeline: how files move between channels and agents

Answers: how do OpenClaw, Hermes, Buzz, and ACP itself handle inbound channel
media (images, voice notes, documents) and outbound agent-produced files?
Produced 2026-07-30 by two source-grounded research agents; spec claims
trace to direct agentclientprotocol.com fetches, product claims to official
docs and raw source fetches. Two flagged unverified items are listed at the
end.

## TL;DR

**Files move by path and reference, not through the conversation protocol.**
Every production system converges on the same shape: download inbound media
to a local cache or staged workspace, preprocess it (transcribe audio,
describe images for non-vision models, extract PDF text), then hand the
agent a LOCAL PATH or a native multimodal block. Outbound, the agent
references a path (marker or tool argument) and the adapter ships the bytes
platform-natively. ACP can carry base64 media inline in content blocks, but
nobody pushes large binary through the conversation wire; Buzz even keeps
media in a separate object-store crate entirely.

## The protocol layer (ACP)

- Five ContentBlock types, identical in both directions (session/prompt in,
  agent_message_chunk out): **text** (mandatory), **image** (base64 `data` +
  `mimeType`, gated by `promptCapabilities.image`), **audio** (base64,
  gated by `promptCapabilities.audio`), **resource** (embedded, `text` or
  base64 `blob`, gated by `promptCapabilities.embeddedContext`), and
  **resource_link** (`uri` + `name`, optional `mimeType`/`size`, no inline
  bytes, ungated). promptCapabilities are agent-declared booleans, default
  false.
- `fs/read_text_file` and `fs/write_text_file` are the ONLY fs methods and
  both are text-only. **No binary file RPC exists.** Binary moves inline as
  base64 blocks or out-of-band (shared workspace, object store) by adapter
  convention. The v2 draft removes the client-owned fs surface entirely.
- No size limits, chunking, or large-file guidance anywhere in the spec:
  the ceiling is the transport (for trogonai: NATS payload limits, ADR 0011
  territory).
- No artifact concept: nothing distinguishes conversational media from a
  durable, downloadable agent output. No download-URL convention either
  (`resource_link.uri` can hold a presigned https URL only by private
  convention).

## Inbound pipeline (channel to agent)

| Step | OpenClaw | Hermes |
|---|---|---|
| Download | temp file per attachment; staged into `media/inbound/*` in the active workspace (path rewritten under Docker sandbox) | platform adapters call shared cache primitives (`cache_media_bytes()`), landing in `~/.hermes/cache/<kind>/<hash>` |
| Size caps | per-channel `mediaMaxMb` (WhatsApp 50, Telegram 100); image understanding 10MB; audio 20MB; PDF 10MB/20 pages | Telegram Bot API 20MB (2GB self-hosted); relay plane 25MB |
| Voice | automatic STT by default, cascade: active model, configured provider (Groq/OpenAI/Deepgram/Google/ElevenLabs/...), local CLIs (whisper-cli, sherpa-onnx, parakeet-mlx); transcript replaces `Body`, sets `{{Transcript}}`, marked machine-generated untrusted text | automatic STT, cascade: local faster-whisper, Groq Whisper, OpenAI Whisper; `stt.enabled:false` still caches audio for agent-side handling |
| Images | vision-capable model: raw image passed as native multimodal block; otherwise a textual `[Image]` description block | same branch: native attach for vision models, pre-analyzed text description otherwise |
| Documents | dedicated `pdf` tool: raw PDF bytes natively to Anthropic/Google, else text extraction with page-render PNG fallback | text docs: extracted text included; other types: saved local path plus a short note |

How it reaches the agent: OpenClaw exposes `{{AttachmentPath}}`,
`{{AttachmentUrl}}`, `{{AttachmentContentType}}`, `{{AttachmentDir}}`
template variables plus a `media://inbound/<id>` URI scheme for tools.
Hermes states it directly: "the agent's vision/file tools consume LOCAL
paths, matching every native adapter's inbound media behaviour."

ACP bridges: telegram-acp-bot supports Telegram image/document attachments
resolved as `file://` resources inside the active workspace (no voice
support). Buzz keeps media out of ACP entirely: a separate `buzz-media`
crate speaks the Blossom protocol (BUD-01/02) against S3/MinIO, independent
of the buzz-acp conversation driver.

## Outbound pipeline (agent to channel)

- **OpenClaw**: the `message(action="send")` tool with `mediaUrl`/`path`
  arguments and per-platform flags (`asVoice`, `asVideoNote`,
  `forceDocument`, Discord `media-gallery` and `attachment://<filename>`
  blocks). WhatsApp auto-optimizes images to fit caps.
- **Hermes**: a text-marker mechanism: the gateway extracts
  `MEDIA:/path/to/file` tags from agent replies and ships the file as a
  platform-native attachment, against an explicit extension allowlist
  (images/audio/video/docs/office/archives). `[SILENT]`/`NO_REPLY` markers
  suppress delivery. The relay plane uploads bytes to `/relay/media` and
  passes a `/relay/media/{id}` reference so local artifacts cross without a
  public URL.
- **telegram-acp-bot**: resolves ACP `file://` resources pointing into the
  workspace and sends them as attachments.

## Workspace and persistence

- Neither system gives each channel session its own workspace: OpenClaw has
  one shared workspace per agent (`~/.openclaw/workspace` default); Hermes
  caches media globally under `~/.hermes/cache/`.
- Hermes is explicit that raw media does NOT persist across turns: future
  turns carry only what was written into the conversation (description,
  cache path, transcript, response).
- Neither documents a TTL/cleanup policy for cached media. Gap in both.

## Security posture

- OpenClaw: sandbox staging with path rewrite; bind-mount validation
  resolves symlinks through the deepest existing ancestor and fails closed
  against `/etc`, `/proc`, `~/.ssh`, `~/.aws`, Docker sockets; sandboxing
  explicitly described as "not a perfect security boundary."
- Hermes: SSRF guard on media downloads by default
  (`security.allow_private_urls: false` rejects RFC 1918, loopback,
  link-local, CGNAT, and cloud-metadata destinations), motivated explicitly
  by prompt-injected URLs; Unicode-lookalike host guard always on.
- Both mark transcripts/metadata as untrusted text. NEITHER documents
  malware scanning of inbound media nor content-level prompt-injection
  scanning of uploaded documents/images. Industry-wide gap.

## trogonai design recommendation

1. **Object store, not protocol payloads**: mirror Buzz's pattern; a media
   service (S3/MinIO) holds inbound channel media and outbound artifacts.
   Never push large base64 through ACP-over-NATS conversation subjects.
2. **References on the wire**: represent stored objects as ACP
   `resource_link` (uri/mimeType/size) or workspace-relative paths staged
   under a `media/inbound/*` convention (OpenClaw pattern; also what the
   sandbox path-rewrite expects).
3. **Preprocessing step in the adapter**: STT cascade for voice (local
   whisper first, API fallback), vision-branch (raw block for multimodal
   models, generated description otherwise), PDF native-vs-extract branch.
   Mark all derived text as untrusted, consistent with the
   [untrusted-context envelope](./bridge-mechanics.md) pattern.
4. **Invent the artifact distinction**: ACP has none; trogonai's adapter
   layer should distinguish inline conversational media from durable
   artifacts (download surface, retention policy), since the protocol will
   not do it.
5. **Outbound via explicit reference, not marker scraping**: prefer a
   send-file tool or resource_link in the final update over Hermes-style
   `MEDIA:` text markers, which are grep-fragile; but keep an extension
   allowlist either way.
6. **Guardrails**: per-channel size caps as config, NATS-layer payload
   limits (ADR 0011), SSRF guard on any URL-fetch path, symlink-fail-closed
   workspace staging, media TTL/cleanup policy (both products lack one;
   trogonai should not).

## Flagged as unverified (do not cite as fact)

- Gemini CLI's actual initialize-response promptCapabilities values
  (image/audio/embeddedContext booleans) could not be traced to a primary
  source.
- claude-agent-acp's exact ContentBlock-to-Claude-API mapping code was not
  readable this pass; its README does list "Images" as a supported feature.

## Sources

Spec: agentclientprotocol.com/protocol/content, /protocol/schema,
/protocol/file-system. OpenClaw: docs.openclaw.ai/nodes/images,
/nodes/audio, /tools/pdf, /tools/media-overview, /channels/{whatsapp,
telegram,discord}, /gateway/config-agents, /gateway/sandboxing,
/concepts/agent-workspace. Hermes: gateway/relay/media.py,
gateway/platforms/media_cache.py, website user-guide (messaging/*,
security.md, sessions.md). Bridges: github.com/mgaitan/telegram-acp-bot,
github.com/block/buzz (buzz-media crate), github.com/agentclientprotocol/
claude-agent-acp, gemini-cli docs/cli/acp-mode.md. See also
[Channel Bridge Mechanics](./bridge-mechanics.md) and the
product dossiers under `./products/`.
