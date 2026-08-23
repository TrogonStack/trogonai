# Session Title and Preview

The first prompt gives a session a name. A later rename overrides it. A redaction
removes the message the first name came from. This page documents the precedence
that resolves a title, why a preview invalidates differently from a stored
string, and what absence means. It documents the protobuf contract that exists
today. There is no Rust implementation yet.

See [Session Query Contract](./session-queries.md) for where these appear and
[Session Presentation Caches](./session-presentation-cache.md) for the
coordinates they invalidate against.

## The precedence, published

The projection resolves a title in one order, always:

1. the latest effective `SessionRenamed`, if one survives
2. otherwise a title derived from the first effective user message
3. otherwise a server-supplied fallback

`TitleSource` says which rule fired. That is what makes the result checkable, and
it lets a client style a guess differently from a name a human chose.

`title` is always set. When there is nothing to show it holds the fallback rather
than an empty string, because a picker that has to invent a name is a picker
where two clients invent different ones.

## The two sources invalidate differently

This asymmetry is why `TitleSource` is an enum and not a `title_is_explicit`
bool.

A derived title is a function of effective history. Rewinding past the first user
message changes it. Redacting that message changes it. It is not a stored value
that happens to have been computed once; it is a projection that has to be
recomputed whenever effective history moves.

An explicit title is an authoritative fact of its own. `SessionRenamed` is an
event, and rewinding or redacting the message that inspired the original name
does not touch it.

What an explicit title does not survive is being redacted itself. When that
happens the resolution falls back down the same precedence rather than keeping a
name whose source no longer exists, and `TitleSource` changes to say so. A title
that outlived its own source event would be exactly the case where a redaction
did not take.

## Preview follows privacy, not just recency

`SessionPreview` is derived from effective history, and that is the whole reason
it belongs in the projection rather than in a column.

Rewind masks ordinals. Redaction removes content from ordinals that stay. Either
can take away the very message a preview was cut from. A preview cached without
regard for that puts deliberately destroyed text back on a screen, which is a
privacy failure and not a staleness annoyance.

Its validity therefore tracks `effective_history_revision` and
`privacy_revision`, the same two coordinates a
[presentation cache](./session-presentation-cache.md) binds against. A cached
preview whose binding fails those two must be discarded, not shown as merely
older. This is the same argument that makes `CACHE_USABILITY_APPENDABLE` gate on
the revisions rather than on the watermark: a lower watermark is only a prefix of
a higher one if history was append-only in between, and none of these operations
is.

## Absence has several causes and they are not the same

`SessionPreview` is always set, including when there is no preview, because the
reason there is none is what a caller needs in order to render the row honestly.

| Availability | What the row should say |
| --- | --- |
| `AVAILABLE` | the excerpt |
| `EMPTY` | nothing was authored |
| `NON_TEXTUAL` | an image-only or attachment-only opening |
| `REDACTED` | content existed and was deliberately removed |
| `WITHHELD` | content exists and this caller may not see it |

Collapsing these into an empty string tells a user a session is empty when it may
be one they are not cleared to see, or one whose opening was removed on purpose.
"No messages" is a lie about both.

`WITHHELD` is per caller, like `ArtifactCompletenessView.hidden`: two readers of
the same session can get different answers, and the difference is authorization
rather than content. `truncated` marks a cut excerpt, so a renderer never
presents a fragment as a whole message.

## Layout

`proto/trogonai/session/sessions/queries/v1alpha1/session_view.proto`

| Type | What it settles |
| --- | --- |
| `TitleSource` | which precedence rule produced the title, and how it invalidates |
| `SessionPreview` | the excerpt, and whether it is a cut |
| `SessionPreviewAvailability` | why there is no excerpt |

`SessionPreview` is on `SessionSummary`, so it is on the list row. That is
deliberate for the same reason `RecoveryProvenanceView` is: a picker is where the
decision gets made, and information that only appears after opening a session
arrives after it stopped mattering.

## Status

Shipped: the preview types and the documented precedence, lint-clean, formatted,
building, and generating Rust bindings reachable at
`trogonai_proto::session::sessions::queries_v1alpha1`.

Not shipped: the projection that derives titles and previews, and the truncation
bound it applies.
