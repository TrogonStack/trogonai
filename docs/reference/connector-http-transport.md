# Connector HTTP Transport

`FINITE_BUFFERED_HTTPS_V1` is the ordinary connection relay's transport profile.
The immutable ConnectorRelease and every admitted `ProxyBinding` name it
explicitly. These contracts specify implementation requirements; they do not
claim a running transport. The [relay reference](./connection-http-proxy.md)
defines request proof and provenance, while [binding integration](./connector-binding-integration.md)
defines destination selection.

## Supported traffic

The profile carries finite HTTPS requests and responses, fully buffered within
the configured bounds. HTTP/1.1 or HTTP/2 may frame that traffic; a streaming
socket implementation does not make application streaming supported. The TLS
client verifies the provider certificate and original hostname. HTTP downgrade,
CONNECT, TRACE, protocol upgrades, WebSocket, SSE and other indefinite responses
are refused. Trusted MCP uses its separately bounded executor profile.

An upload is completely received and its content digest verified before custody
resolution or upstream dispatch. A response is completely received and checked
before any headers or body are released to the workload. Exceeding a limit aborts
the operation; a partial provider response is never returned as a successful
empty response. No request or response body is persisted for recovery.

## Bounds and accounting

These values are platform policy choices, selected to bound memory and request
occupancy rather than derived from provider quotas. An operator may lower them.
Raising a default requires an audited platform configuration change, remains
within the absolute ceiling and does not widen any origin or permission.

| Bound | Platform default | Absolute ceiling |
| --- | --- | --- |
| Request content | 256 KiB | 1 MiB |
| Response content | 2 MiB | 8 MiB |
| Request header section | 16 KiB | 16 KiB |
| Response header section | 16 KiB | 16 KiB |
| Header fields per message | 128 | 128 |
| Request target, path and query | 8 KiB | 8 KiB |
| DNS, TCP and TLS connection establishment combined | 5 seconds | 5 seconds |
| Entire relay operation | 30 seconds | 30 seconds |
| Concurrent operations per grant | 4 | 4 |

A release's `ProxyLimits` may only lower the current platform value. Omitted
fields inherit it; supplying a value below an absolute schema ceiling does not
authorize exceeding the current platform value. The effective deadline is the
earliest of the selected total deadline, proof expiry, grant expiry, Session or
attempt end, and cancellation. The total timer starts at relay ingress and
includes upload, queues, name resolution, custody, upstream transfer and checks.
The connection timer is also bounded by the remaining total deadline.

Content bytes are counted after HTTP transfer framing is removed. Header bytes
include each name, separator, value and line delimiter, plus the terminating
delimiter; HTTP/2 uses the same uncompressed logical accounting. Count duplicate
field lines separately before combining any allowed list. Relay context/proof
fields count toward ingress header bounds, with the separate 4 KiB decoded
context limit still applying. Enforce a process-wide concurrency and memory
budget in addition to the grant cap; admission refuses capacity rather than
allocating an unbounded waiting queue.

## Content coding and representation policy

Version 1 accepts no `Content-Encoding` on either leg, including an explicit
`identity` coding. The supervisor supplies and signs `Accept-Encoding: identity`
and refuses a workload override. Both HTTP clients disable automatic content
decoding. A provider response with a content coding fails before release even if
the library could decode it. This makes the encoded and decoded content bounds
identical: there is no decompressor, expansion-ratio allowance or compressed
representation bypass in this profile. Supporting gzip, Brotli or stacked
codings requires another reviewed profile with limits on wire bytes, decoded
bytes, expansion and decoder work.

These coding rules are a deliberately restrictive project policy. HTTP defines
content coding separately from transfer framing and reserves `identity` for
`Accept-Encoding`; ordinary HTTP does not require this project's restriction.
[HTTP content coding](https://www.rfc-editor.org/rfc/rfc9110.html#section-8.4)

This profile does not decode archives, embedded encodings or arbitrary provider
formats. Literal credential detection is a baseline, not a proof against every
transformation a provider could return. A provider requiring stronger protection
must pin a reviewed trusted response filter before admission; absent that filter,
the provider/profile combination is unsupported.

## Framing and completion

The transport adapter uses a strict HTTP parser. It refuses conflicting length
and transfer framing, duplicate Content-Length fields even when equal, obsolete
line folding, invalid field bytes and declared or actual trailers. Only the
HTTP/1.1 `chunked` transfer coding may be decoded, within the same content,
header and deadline bounds; chunk extensions are refused. An HTTP/1.1 response
that requires connection-close delimiting is unsupported. Outbound authority,
Content-Length and hop-by-hop fields are rebuilt from the validated buffered
message. The strict subset reduces parser differences that can enable request
smuggling. [HTTP/1.1 framing](https://www.rfc-editor.org/rfc/rfc9112.html#section-6.3)

GET and HEAD request bodies are refused. The supervisor handles `100 Continue`
locally and does not forward a workload `Expect` field. Upstream informational
responses are consumed locally, never treated as completion, and limited to four
within the same aggregate header/deadline budget. `101 Switching Protocols` is
unsupported. SSE is refused by its parsed `text/event-stream` media type before
reading a body. Request `Range` and `If-Range`, upstream `206`, Content-Range and
`multipart/byteranges` are unsupported; version 1 provides no range assembly.

HEAD, 204 and 304 responses carry no content under their HTTP semantics. A valid
Content-Length on HEAD or 304 describes the selected representation and is not
itself evidence of a payload. Retain that metadata when HTTP permits it; do not
replace it with a fabricated zero. A 204 must have no Content-Length or
Transfer-Encoding. A 205 response must have zero content. Illegal framing or
observed illegal content on a bodyless response fails the profile. HTTP/1.1
connections are closed after bodyless responses and any residual bytes are
discarded; completion does not wait for a forbidden body. This prevents stray
bytes from becoming another response without claiming that an HTTP/1.1 parser
can identify every byte arriving after its defined message boundary. HTTP/2
must enforce the corresponding no-content stream semantics. [HTTP message
content](https://www.rfc-editor.org/rfc/rfc9110.html#section-6.4)

## Refusal and effect uncertainty

Unsupported or malformed workload requests produce the relay's safe
`INPUT_REJECTED` failure before custody or provider dispatch. A response that
violates this profile produces `PROVIDER_RESPONSE_REJECTED` with safe HTTP 502
and no provider content. A response exceeding its body cap uses
`RESPONSE_TOO_LARGE`. An interrupted send or incomplete attributable response
uses the recovery protocol's unknown-outcome rules.

A rejected response does not establish that the provider did nothing. The
transport never turns an unsupported response, timeout or lost reply into an
automatic resend. Recovery and audit record that distinction. Schema validation
cannot prove parser strictness, byte accounting, address pinning, bodyless
handling or runtime memory bounds; those need transport conformance tests.
