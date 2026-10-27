import { CredentialBindingLifecycle } from '@trogonai/proto';
import { useState, type ReactNode } from 'react';

import { CopyButton } from '@/components/copy-button';
import { Digest } from '@/components/digest';
import { FaultAlert } from '@/components/fault-alert';
import { BindingLifecycleBadge, bindingTone, toneVariable } from '@/components/lifecycle-badge';
import { ScrambleText } from '@/components/scramble-text';
import { Button } from '@/components/ui/button';
import { useNotify } from '@/components/ui/toaster';
import { cn } from '@/lib/utils';

import { describeAuthentication, isActiveConnection, type BindingView, type ConnectionView } from '../domain/view';
import { useEndPinnedSessions, useRevokeBinding } from '../queries';
import { ReplaceBindingDialog } from './replace-binding-dialog';

const meterSegments = 12;

export function PinnedMeter({ pinned, draining }: { pinned: number; draining: boolean }) {
  const lit = Math.min(pinned, meterSegments);
  return (
    <div className="flex items-center gap-2.5">
      <div
        role="meter"
        aria-label="Pinned sessions"
        aria-valuemin={0}
        aria-valuemax={meterSegments}
        aria-valuenow={lit}
        className={cn('flex h-3.5 items-end gap-[3px]', draining ? '[--led:var(--info)]' : '[--led:var(--success)]')}
      >
        {Array.from({ length: meterSegments }, (_, index) => (
          <span
            key={index}
            aria-hidden="true"
            className={cn(
              'h-full w-1 rounded-[1px]',
              index < lit ? 'led' : 'bg-muted-foreground/20',
              index < lit && draining && 'led-blink',
            )}
            style={index < lit && draining ? { animationDelay: `${index * 90}ms` } : undefined}
          />
        ))}
      </div>
      <span className="font-mono text-sm font-medium tabular-nums">{pinned}</span>
    </div>
  );
}

function Field({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="flex min-w-0 flex-col gap-1">
      <dt className="micro text-muted-foreground text-[10px]">{label}</dt>
      <dd className="min-w-0 text-[13px]">{children}</dd>
    </div>
  );
}

function BindingCard({ connection, binding }: { connection: ConnectionView; binding: BindingView }) {
  const revoke = useRevokeBinding();
  const endSessions = useEndPinnedSessions();
  const notify = useNotify();
  const [replacing, setReplacing] = useState(false);

  const connectionActive = isActiveConnection(connection);
  const canReplace = connectionActive && binding.lifecycle === CredentialBindingLifecycle.ACTIVE;
  const canRevoke = connectionActive && binding.lifecycle !== CredentialBindingLifecycle.REVOKED;
  const revoked = binding.lifecycle === CredentialBindingLifecycle.REVOKED;
  const secretRef = 'secretRef' in binding.authentication ? binding.authentication.secretRef : undefined;

  return (
    <li
      className={cn(
        'relative flex flex-col gap-4 overflow-hidden rounded-lg border border-border bg-card p-4 pl-5 shadow-[inset_0_1px_0_0_var(--highlight)]',
        toneVariable(bindingTone(binding.lifecycle)),
        revoked && 'opacity-70',
      )}
    >
      <span aria-hidden="true" className={cn('absolute inset-y-0 left-0 w-1 bg-(--led)', revoked && 'opacity-40')} />
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div className="flex flex-wrap items-center gap-3">
          <ScrambleText
            text={binding.id}
            className={cn('text-sm font-medium', revoked && 'line-through decoration-muted-foreground/60')}
          />
          <BindingLifecycleBadge lifecycle={binding.lifecycle} />
        </div>
        <div className="flex flex-wrap items-center justify-end gap-2">
          {canReplace ? (
            <Button type="button" variant="outline" size="sm" onClick={() => setReplacing(true)}>
              Replace binding
            </Button>
          ) : null}
          {canRevoke ? (
            <Button
              type="button"
              variant="destructive"
              size="sm"
              aria-label={`Revoke binding ${binding.id}`}
              disabled={revoke.isPending}
              onClick={() =>
                revoke.mutate(
                  { connectionId: connection.id, expectedRevision: connection.revision, bindingId: binding.id },
                  {
                    onSuccess: () => notify.success('Binding revoked', binding.id),
                    onError: (error) => notify.failure('Binding was not revoked', error.message),
                  },
                )
              }
            >
              Revoke binding
            </Button>
          ) : null}
        </div>
      </div>
      <dl className="grid grid-cols-1 gap-x-6 gap-y-3 sm:grid-cols-2">
        <Field label="Authentication">{describeAuthentication(binding.authentication)}</Field>
        {secretRef ? (
          <Field label="Secret reference">
            <span className="inline-flex max-w-full items-center gap-1 font-mono text-xs">
              <span className="break-all">{secretRef}</span>
              <CopyButton value={secretRef} label="secret reference" />
            </span>
          </Field>
        ) : null}
        <Field label="Digest">
          <Digest hex={binding.digestHex} label="binding digest" />
        </Field>
        <Field label="Pinned sessions">
          <div className="flex flex-wrap items-center gap-3">
            <PinnedMeter
              pinned={binding.pinnedSessions}
              draining={binding.lifecycle === CredentialBindingLifecycle.DRAINING}
            />
            {binding.pinnedSessions > 0 ? (
              <Button
                type="button"
                variant="ghost"
                size="xs"
                disabled={endSessions.isPending}
                onClick={() =>
                  endSessions.mutate(
                    { connectionId: connection.id, bindingId: binding.id },
                    { onSuccess: () => notify.success('Pinned sessions ended', binding.id) },
                  )
                }
              >
                Simulate sessions ending
              </Button>
            ) : null}
          </div>
        </Field>
      </dl>
      <FaultAlert error={revoke.error ?? endSessions.error} title="The command was rejected" />
      {canReplace ? (
        <ReplaceBindingDialog connection={connection} previous={binding} open={replacing} onOpenChange={setReplacing} />
      ) : null}
    </li>
  );
}

export function BindingsPanel({ connection }: { connection: ConnectionView }) {
  return (
    <ul aria-label="Credential bindings" className="flex flex-col gap-3">
      {connection.bindings.map((binding) => (
        <BindingCard key={binding.id} connection={connection} binding={binding} />
      ))}
    </ul>
  );
}
