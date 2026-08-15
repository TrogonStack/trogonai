# Retire the ACP Notifications Stream

`session/update` reaches the client through the client-op proxy for every
operation. The prompt-scoped notification path that used the
`<PREFIX>_NOTIFICATIONS` stream was a second delivery path for the same
updates, so it was removed, and the ACP provisioner no longer creates that
stream.

Removing a stream from the provisioner does not remove it from a deployment
that already ran an earlier release. That deployment still has the stream, its
stored messages, and its storage bill. This page is how you retire it.

## When to use this

Use this procedure once per ACP deployment, after every process has been
upgraded to a release whose provisioner no longer lists the stream. Check the
deployed process versions to establish that, then use this to see whether the
retired stream is still there:

```shell
nats stream ls
```

You are retiring a stream whose name matches `retired_stream_names` for your
prefix. For the default `acp` prefix that is `ACP_NOTIFICATIONS`; for a prefix
of `my.multi.part` it is `MY_MULTI_PART_NOTIFICATIONS`.

Do not use this procedure while any process is still running the previous
release. Those processes publish to `<prefix>.v1.session.*.agent.update`, and
deleting the stream underneath them drops those messages.

## Preconditions

- Every ACP agent and client process runs a release that does not provision
  the stream. A mixed fleet is the one case where this procedure loses data.
- The stream has no consumers with unacknowledged messages you still need.
  `nats consumer ls <PREFIX>_NOTIFICATIONS` lists them; an empty list is the
  state you want before deleting.
- You have a JetStream account credential with delete authority on the
  stream.

## Steps

1. Confirm the stream is idle. Its message count should stop growing:

   ```shell
   nats stream info <PREFIX>_NOTIFICATIONS
   ```

   A message count that still climbs means something is publishing to
   `<prefix>.v1.session.*.agent.update`. Find it and upgrade it before
   continuing.

2. Delete the remaining consumers. Deleting the stream removes them anyway,
   but doing it first makes a still-attached reader fail visibly here rather
   than silently later:

   ```shell
   nats consumer rm <PREFIX>_NOTIFICATIONS <CONSUMER>
   ```

3. Delete the stream:

   ```shell
   nats stream rm <PREFIX>_NOTIFICATIONS
   ```

## What this does not do

The provisioner does not perform any of the above. It creates and reconciles
the streams it declares and touches nothing else, so a stream delete is never
a side effect of a boot. That is deliberate: a stream delete is
unrecoverable, it races an operator who may still be draining the stream, and
a rollback to the previous release would re-create the stream empty and hide
the fact that its history is gone.

## Rollback

There is none. A deleted stream and its messages do not come back. If you
roll back to a release that still provisions the stream, the provisioner
creates it again with no history.
