import { Link } from '@tanstack/react-router';
import { OperationFailureCategory } from '@trogonai/proto';
import { ShieldAlertIcon } from 'lucide-react';
import type { ReactNode } from 'react';

import { StatusPill } from '@/components/lifecycle-badge';
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card';
import { RelativeTime } from '@/features/secrets/components/relative-time';
import { formatTimestamp } from '@/features/secrets/components/time';

import { keyCapabilityLabels } from '../domain/capability';
import { failureExplanation, failureLabel, failureSemantics } from '../domain/failures';
import { denialReasonMessages } from '../domain/faults';
import { keyStateLabels, keyStateSummaries } from '../domain/key-state';
import {
  operationKindLabels,
  providerRequestAbsenceLabels,
  providerRequestIdNote,
  type AttemptView,
  type UsageRecordView,
} from '../domain/usage';
import type { KeyView } from '../domain/view';
import { KeyStateBadge, PlatformManagedBadge } from './key-badges';
import { UsageChart } from './usage-chart';

function Fact({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="flex flex-col gap-1">
      <dt className="micro text-muted-foreground text-[10px]">{label}</dt>
      <dd className="text-[13px] break-all">{children}</dd>
    </div>
  );
}

export function KeyFactsPanel({ view }: { view: KeyView }) {
  return (
    <Card>
      <CardHeader>
        <CardTitle>Key</CardTitle>
      </CardHeader>
      <CardContent>
        <dl className="grid gap-4 sm:grid-cols-2">
          <Fact label="Backend">
            {view.customer ? (
              <Link
                to="/keys/backends/$backendId"
                params={{ backendId: view.customer.backend.id }}
                className="hover:underline focus-visible:underline"
              >
                {view.customer.backend.displayName}
              </Link>
            ) : (
              <PlatformManagedBadge />
            )}
          </Fact>
          <Fact label="Purpose">{view.purposeLabel}</Fact>
          <Fact label="Capabilities">
            {view.capabilities.map((capability) => keyCapabilityLabels[capability]).join(', ')}
          </Fact>
          <Fact label="Bound">{formatTimestamp(view.boundAt)}</Fact>
          {view.customer ? (
            <Fact label="Provider locator">
              <span className="font-mono text-xs">{view.customer.locator}</span>
              <span className="text-muted-foreground block text-xs">
                Visible only to you, so you can match this binding to the key in your provider console.
              </span>
            </Fact>
          ) : null}
        </dl>
      </CardContent>
    </Card>
  );
}

export function KeyStatePanel({ view }: { view: KeyView }) {
  const paused = view.blockedByPause;
  const failingClosed = !paused && view.unwrapDenial !== undefined;
  return (
    <Card>
      <CardHeader>
        <div className="flex flex-col gap-1">
          <CardTitle>State</CardTitle>
          <CardDescription>
            What your provider reports for the key, last observed <RelativeTime date={view.observedAt} />.
          </CardDescription>
        </div>
      </CardHeader>
      <CardContent className="flex flex-col gap-4">
        <div className="flex flex-col gap-1.5">
          <KeyStateBadge state={view.state} />
          <p className="text-muted-foreground text-xs">{keyStateSummaries[view.state]}</p>
        </div>
        {paused ? (
          <p
            role="status"
            className="border-warning/35 bg-warning-soft text-warning-ink rounded-lg border px-3 py-2.5 text-[13px]"
          >
            {denialReasonMessages[view.unwrapDenial!]} Nothing wrapped under this key is read or written meanwhile.
          </p>
        ) : null}
        {failingClosed ? (
          <p
            role="status"
            className="border-danger/30 bg-danger-soft text-danger-ink flex items-start gap-2 rounded-lg border px-3 py-2.5 text-[13px]"
          >
            <ShieldAlertIcon className="mt-0.5 size-4 shrink-0" aria-hidden="true" />
            The platform fails closed. Nothing wrapped under this key can be decrypted while it is{' '}
            {keyStateLabels[view.state].toLowerCase()}.
          </p>
        ) : null}
      </CardContent>
    </Card>
  );
}

export function UsagePanel({ view }: { view: KeyView }) {
  return (
    <Card>
      <CardHeader>
        <div className="flex flex-col gap-1">
          <CardTitle>Wraps and unwraps</CardTitle>
          <CardDescription>
            {view.wraps7d} wraps and {view.unwraps7d} unwraps in the last 7 days.
          </CardDescription>
        </div>
      </CardHeader>
      <CardContent>
        <UsageChart usage={view.usage} />
      </CardContent>
    </Card>
  );
}

export function PayloadsPanel({ view }: { view: KeyView }) {
  return (
    <Card>
      <CardHeader>
        <div className="flex flex-col gap-1">
          <CardTitle>Payloads wrapped under this key</CardTitle>
          <CardDescription>
            {view.payloadCount === 0
              ? 'Nothing is wrapped under this key.'
              : `${view.payloadCount} payloads are wrapped under this key.`}
          </CardDescription>
        </div>
      </CardHeader>
      <CardContent className="flex flex-col gap-3">
        {view.payloads.length > 0 ? (
          <ul className="divide-border divide-y" aria-label="Payload sets">
            {view.payloads.map((set) => (
              <li key={set.id} className="flex items-center gap-3 py-2 first:pt-0 last:pb-0">
                <span className="min-w-0 flex-1 truncate text-[13px]">{set.label}</span>
                <span className="font-mono text-xs tabular-nums">{set.count}</span>
              </li>
            ))}
          </ul>
        ) : null}
        {view.inbound > 0 ? (
          <p className="text-muted-foreground text-xs">
            {view.inbound} payloads have already been rewrapped to this key by a running migration.
          </p>
        ) : null}
        {view.outbound > 0 ? (
          <p className="text-muted-foreground text-xs">
            {view.outbound} payloads have already been moved away from this key by a running migration.
          </p>
        ) : null}
      </CardContent>
    </Card>
  );
}

function ProviderRequest({ attempt }: { attempt: AttemptView }) {
  const request = attempt.providerRequest;
  return request.kind === 'id' ? (
    <span className="font-mono text-xs break-all">{request.id}</span>
  ) : (
    <span className="text-muted-foreground text-xs">{providerRequestAbsenceLabels[request.reason]}</span>
  );
}

function attemptOutcome(attempt: AttemptView): string {
  return attempt.outcome.kind === 'succeeded' ? 'Succeeded' : failureSemantics[attempt.outcome.category].label;
}

function UsageRecordItem({ record }: { record: UsageRecordView }) {
  return (
    <li className="flex flex-col gap-2 py-3 first:pt-0 last:pb-0">
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
        <span className="text-[13px] font-medium">{operationKindLabels[record.operation]}</span>
        {record.outcome.kind === 'succeeded' ? (
          <StatusPill tone="success" label="Succeeded" />
        ) : (
          <StatusPill
            tone={record.outcome.failure.category === OperationFailureCategory.KEY_USE_DENIED ? 'warning' : 'danger'}
            label={failureLabel(record.outcome.failure)}
            className="normal-case"
          />
        )}
        <RelativeTime date={record.occurredAt} className="text-muted-foreground ml-auto text-xs" />
      </div>
      {record.outcome.kind === 'failed' ? (
        <p className="text-muted-foreground text-xs">{failureExplanation(record.outcome.failure)}</p>
      ) : null}
      {record.attempts.length === 0 ? (
        <p className="text-muted-foreground text-xs">The platform made no provider call.</p>
      ) : (
        <ol aria-label={`Provider attempts for ${record.operationId}`} className="flex flex-col gap-1">
          {record.attempts.map((attempt) => (
            <li
              key={attempt.number}
              className="bg-surface-deep/40 border-border flex flex-wrap items-center gap-x-4 gap-y-1 rounded-md border px-3 py-1.5"
            >
              <span className="micro text-muted-foreground text-[10px]">Attempt {attempt.number}</span>
              <span className="text-muted-foreground text-xs">{formatTimestamp(attempt.startedAt)}</span>
              <span className="text-muted-foreground font-mono text-xs tabular-nums">{attempt.durationMs} ms</span>
              <span className="text-xs">{attemptOutcome(attempt)}</span>
              <span className="flex min-w-0 flex-1 items-center justify-end gap-2">
                <span className="micro text-muted-foreground text-[10px]">Provider request</span>
                <ProviderRequest attempt={attempt} />
              </span>
            </li>
          ))}
        </ol>
      )}
      <span className="text-muted-foreground font-mono text-[11px]">{record.operationId}</span>
    </li>
  );
}

export function UsageRecordsPanel({ view }: { view: KeyView }) {
  return (
    <Card>
      <CardHeader>
        <div className="flex flex-col gap-1">
          <CardTitle>Usage records</CardTitle>
          <CardDescription>
            Each operation lists the provider attempts the platform made. {providerRequestIdNote} When the provider
            returned none, the reason is shown instead.
          </CardDescription>
        </div>
      </CardHeader>
      <CardContent>
        {view.records.length === 0 ? (
          <p className="text-muted-foreground text-[13px]">No operation has used this key yet.</p>
        ) : (
          <ul className="divide-border divide-y" aria-label="Usage records">
            {view.records.map((record) => (
              <UsageRecordItem key={record.operationId} record={record} />
            ))}
          </ul>
        )}
      </CardContent>
    </Card>
  );
}
