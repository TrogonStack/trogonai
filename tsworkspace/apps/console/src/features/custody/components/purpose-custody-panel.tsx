import { ShieldAlertIcon } from 'lucide-react';
import type { ReactNode } from 'react';

import { TableEmptyState } from '@/components/data-table';
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card';

import { denialReasonMessages } from '../domain/faults';
import type { PurposeCustodyView } from '../domain/view';
import { CustodyTierBadge } from './custody-tier-badge';
import { KeyStateBadge, PlatformManagedBadge } from './key-badges';
import { KeyRefLink } from './key-ref-link';

function Field({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="flex flex-col gap-1">
      <dt className="micro text-muted-foreground text-[10px]">{label}</dt>
      <dd className="text-[13px]">{children}</dd>
    </div>
  );
}

function PurposeCard({ purpose }: { purpose: PurposeCustodyView }) {
  const key = purpose.key;
  return (
    <li
      className="border-border bg-surface-deep/40 flex flex-col gap-3 rounded-lg border p-4"
      aria-label={purpose.purpose.label}
    >
      <div className="flex flex-wrap items-start justify-between gap-2">
        <div className="flex flex-col gap-1">
          <h3 className="text-[15px] font-semibold">{purpose.purpose.label}</h3>
          <p className="text-muted-foreground text-xs">{purpose.purpose.description}</p>
        </div>
        <CustodyTierBadge tier={purpose.tier} />
      </div>
      <dl className="grid gap-3 sm:grid-cols-2">
        <Field label="Key">
          {key ? (
            <span className="flex items-center gap-2">
              <KeyRefLink keyRef={key.keyRef} />
              <KeyStateBadge state={key.state} />
            </span>
          ) : (
            <PlatformManagedBadge />
          )}
        </Field>
        <Field label="Backend">{key?.customer ? key.customer.backend.displayName : <PlatformManagedBadge />}</Field>
        <Field label="Payloads">
          <span className="font-mono tabular-nums">{purpose.payloadCount}</span>
        </Field>
        <Field label="Owned by">{purpose.purpose.owningService}</Field>
      </dl>
      {purpose.denial !== undefined ? (
        <p
          role="status"
          className="border-danger/30 bg-danger-soft text-danger-ink flex items-start gap-2 rounded-lg border px-3 py-2 text-xs"
        >
          <ShieldAlertIcon className="mt-0.5 size-4 shrink-0" aria-hidden="true" />
          {denialReasonMessages[purpose.denial]} The platform fails closed for this purpose.
        </p>
      ) : null}
      {purpose.migration ? (
        <p className="text-muted-foreground text-xs">
          A migration is running: {purpose.migration.percent}% of payloads have been rewrapped.
        </p>
      ) : null}
    </li>
  );
}

export function PurposeCustodyPanel({ purposes }: { purposes: readonly PurposeCustodyView[] }) {
  return (
    <Card>
      <CardHeader>
        <div className="flex flex-col gap-1">
          <CardTitle>Current custody</CardTitle>
          <CardDescription>
            Who holds the key that wraps each kind of data. A default applies to new payloads only.
          </CardDescription>
        </div>
      </CardHeader>
      <CardContent>
        {purposes.length === 0 ? (
          <TableEmptyState
            title="No key purposes yet"
            message="Purposes appear here once a platform service starts encrypting your data with a key."
          />
        ) : (
          <ul className="grid gap-4 lg:grid-cols-2">
            {purposes.map((purpose) => (
              <PurposeCard key={purpose.purpose.id} purpose={purpose} />
            ))}
          </ul>
        )}
      </CardContent>
    </Card>
  );
}
