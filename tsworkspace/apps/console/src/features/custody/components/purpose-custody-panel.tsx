import { Link } from '@tanstack/react-router';
import { ShieldAlertIcon } from 'lucide-react';
import { useState, type ReactNode } from 'react';

import { TableEmptyState } from '@/components/data-table';
import { Button } from '@/components/ui/button';
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card';

import { denialReasonMessages } from '../domain/faults';
import type { CustodyView, PurposeCustodyView } from '../domain/view';
import { ChangeCustodyDialog } from './change-custody-dialog';
import { CustodyTierBadge } from './custody-tier-badge';
import { KeyStateBadge, MigrationStatusBadge, PlatformManagedBadge } from './key-badges';
import { KeyRefLink } from './key-ref-link';
import { MigrationProgressBar } from './migration-progress';

function Field({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="flex flex-col gap-1">
      <dt className="micro text-muted-foreground text-[10px]">{label}</dt>
      <dd className="text-[13px]">{children}</dd>
    </div>
  );
}

function PurposeCard({ data, purpose }: { data: CustodyView; purpose: PurposeCustodyView }) {
  const [changing, setChanging] = useState(false);
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
        <div className="flex flex-col gap-1.5">
          <div className="flex items-center gap-2">
            <MigrationStatusBadge status={purpose.migration.migration.status} />
            <Link
              to="/keys/migrations/$migrationId"
              params={{ migrationId: purpose.migration.migration.id }}
              className="text-primary-ink font-mono text-xs underline"
            >
              {purpose.migration.migration.id}
            </Link>
          </div>
          <MigrationProgressBar view={purpose.migration} compact />
        </div>
      ) : null}
      <div className="flex flex-col items-start gap-1.5">
        <Button
          type="button"
          variant="outline"
          disabled={purpose.migration !== undefined}
          onClick={() => setChanging(true)}
        >
          Change custody
        </Button>
        {purpose.migration ? (
          <p className="text-muted-foreground text-xs">Wait for the running migration to finish first.</p>
        ) : null}
      </div>
      <ChangeCustodyDialog data={data} purpose={purpose.purpose.id} open={changing} onOpenChange={setChanging} />
    </li>
  );
}

export function PurposeCustodyPanel({ data }: { data: CustodyView }) {
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
        {data.purposes.length === 0 ? (
          <TableEmptyState
            title="No key purposes yet"
            message="Purposes appear here once a platform service starts encrypting your data with a key."
          />
        ) : (
          <ul className="grid gap-4 lg:grid-cols-2">
            {data.purposes.map((purpose) => (
              <PurposeCard key={purpose.purpose.id} data={data} purpose={purpose} />
            ))}
          </ul>
        )}
      </CardContent>
    </Card>
  );
}
