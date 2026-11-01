import { Link, useNavigate } from '@tanstack/react-router';
import { KeyBackendAdmission } from '@trogonai/proto';
import { TriangleAlertIcon } from 'lucide-react';
import { useState } from 'react';

import { FaultAlert } from '@/components/fault-alert';
import { RadioCards, SelectField, type RadioCardOption } from '@/components/field';
import { Button } from '@/components/ui/button';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog';
import { useNotify } from '@/components/ui/toaster';

import { custodyTierLabels, type CustodyTier } from '../domain/custody';
import type { PurposeId } from '../domain/identifiers';
import { canWrap } from '../domain/key-state';
import type { CustodyTarget } from '../domain/migration';
import type { CustodyView, KeyView } from '../domain/view';
import { useStartMigration } from '../queries';
import { CustodyMigrationNote } from './custody-migration-note';
import { CustodyTierBadge } from './custody-tier-badge';

interface ChangeCustodyDialogProps {
  data: CustodyView;
  purpose?: PurposeId;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

const targetDescriptions: Record<CustodyTier, string> = {
  managed: 'The default. A key operated by the platform and shared across tenants.',
  dedicated: 'Opt in to a platform-operated key reserved for your tenant and this purpose.',
  customer_managed: 'Use a key from a backend you control. Requires a bound key reference.',
};

function keyOption(key: KeyView): { value: string; label: string } {
  const backend = key.customer?.backend;
  const paused = backend?.admission === KeyBackendAdmission.PAUSED ? ', backend paused' : '';
  return { value: key.keyRef, label: `${key.keyRef} (${backend?.displayName ?? 'platform'}${paused})` };
}

function Consequences({ from, to, count }: { from: CustodyTier; to: CustodyTier; count: number }) {
  const leaving = from === 'customer_managed' && to !== 'customer_managed';
  return (
    <div className="flex flex-col gap-3">
      <div className="flex flex-col gap-1.5">
        <h3 className="micro text-muted-foreground text-[10px]">What changes</h3>
        <ul className="text-muted-foreground flex list-disc flex-col gap-1 pl-4 text-xs">
          <li>The new key becomes the default for new payloads immediately.</li>
          {count === 0 ? (
            <li>No existing payloads are wrapped under the current key, so no background migration will start.</li>
          ) : (
            <li>
              A migration rewraps the {count} existing payloads under the current key in the background. You can pause
              and resume it.
            </li>
          )}
          {to === 'dedicated' ? <li>The platform creates the dedicated key when you confirm.</li> : null}
          <li>
            {leaving
              ? 'Your key and its backend must stay enabled and unpaused until the migration finishes. There is no fallback.'
              : 'Both keys must be available for the whole migration. There is no fallback to the old key.'}
          </li>
        </ul>
      </div>
      <div
        role="note"
        className="border-warning/35 bg-warning-soft text-warning-ink flex items-start gap-2.5 rounded-lg border px-3 py-2.5 text-xs"
      >
        <TriangleAlertIcon className="mt-0.5 size-4 shrink-0" aria-hidden="true" />
        <p>
          {to === 'customer_managed'
            ? 'Shutdown is yours to control and the platform only observes it. If you disable or destroy the key, or revoke the access you granted, the platform fails closed: nothing is decrypted and requests are rejected until the key is back. A destroyed key cannot be recovered by the platform.'
            : 'The platform operates this key, so you cannot disable it yourself. If it becomes unavailable the platform fails closed.'}
        </p>
      </div>
    </div>
  );
}

export function ChangeCustodyDialog({ data, purpose: fixedPurpose, open, onOpenChange }: ChangeCustodyDialogProps) {
  const start = useStartMigration();
  const navigate = useNavigate();
  const notify = useNotify();
  const [purposeChoice, setPurposeChoice] = useState<PurposeId>();
  const [targetChoice, setTargetChoice] = useState<CustodyTier>();
  const [keyChoice, setKeyChoice] = useState<string>();

  const selectable = data.purposes.filter((entry) => entry.migration === undefined);
  const view =
    data.purposes.find((entry) => entry.purpose.id === (fixedPurpose ?? purposeChoice)) ??
    selectable[0] ??
    data.purposes[0];
  if (!view) return null;

  const candidates = data.keys.filter(
    (key) => key.tier === 'customer_managed' && key.purpose === view.purpose.id && !key.isDefault && canWrap(key.state),
  );
  const chosen = candidates.find((key) => key.keyRef === keyChoice) ?? candidates[0];

  const tiers = (['managed', 'dedicated', 'customer_managed'] as const).filter(
    (tier) => tier !== view.tier || tier === 'customer_managed',
  );
  const targets: readonly RadioCardOption<CustodyTier>[] = tiers.map((tier) => ({
    value: tier,
    label: custodyTierLabels[tier],
    description:
      tier === 'customer_managed' && view.tier === 'customer_managed'
        ? 'Move to another key from a backend you control.'
        : targetDescriptions[tier],
  }));
  const target = tiers.find((tier) => tier === targetChoice) ?? tiers[0]!;
  const blocked = target === 'customer_managed' && !chosen;
  const running = view.migration !== undefined;

  const close = (next: boolean) => {
    if (!next) {
      start.reset();
      setPurposeChoice(undefined);
      setTargetChoice(undefined);
      setKeyChoice(undefined);
    }
    onOpenChange(next);
  };

  const submit = () => {
    const resolved: CustodyTarget | undefined =
      target === 'customer_managed'
        ? chosen
          ? { tier: 'customer_managed', keyRef: chosen.keyRef }
          : undefined
        : { tier: target };
    if (!resolved) return;
    start.mutate(
      { purpose: view.purpose.id, target: resolved },
      {
        onSuccess: (result) => {
          notify.success(
            'Custody changed',
            `${view.purpose.label} now uses a ${custodyTierLabels[target].toLowerCase()}.`,
          );
          close(false);
          if (result.migration) {
            void navigate({ to: '/keys/migrations/$migrationId', params: { migrationId: result.migration.id } });
          }
        },
      },
    );
  };

  return (
    <Dialog open={open} onOpenChange={close}>
      <DialogContent className="sm:max-w-xl">
        <DialogHeader>
          <DialogTitle>Change custody for {view.purpose.label.toLowerCase()}</DialogTitle>
          <DialogDescription>
            Today this purpose uses a <CustodyTierBadge tier={view.tier} className="mx-1 align-middle" />. Choose where
            its payloads should be wrapped.
          </DialogDescription>
        </DialogHeader>
        <form
          noValidate
          aria-label="Change custody"
          className="flex flex-col gap-4"
          onSubmit={(event) => {
            event.preventDefault();
            submit();
          }}
        >
          <FaultAlert error={start.error} title="The change was rejected" />
          {fixedPurpose === undefined ? (
            <SelectField
              id="custody-purpose"
              label="Purpose"
              value={view.purpose.id as string}
              options={data.purposes.map((entry) => ({
                value: entry.purpose.id as string,
                label: entry.purpose.label,
              }))}
              onChange={(value) => {
                setPurposeChoice(data.purposes.find((entry) => entry.purpose.id === value)?.purpose.id);
                setTargetChoice(undefined);
                setKeyChoice(undefined);
              }}
            />
          ) : null}
          {running ? (
            <p role="status" className="text-muted-foreground text-[13px]">
              A migration is already running for this purpose. Wait for it to finish first.
            </p>
          ) : null}
          <RadioCards
            name="custody-target"
            label="New custody"
            value={target}
            options={targets}
            onChange={setTargetChoice}
            columns={targets.length === 1 ? 1 : 2}
          />
          {target === 'customer_managed' ? (
            candidates.length === 0 ? (
              <p role="status" className="text-muted-foreground text-[13px]">
                No usable customer managed key is bound for this purpose.{' '}
                <Link to="/keys/keys/new" className="text-primary-ink underline" onClick={() => close(false)}>
                  Bind a key
                </Link>{' '}
                first.
              </p>
            ) : (
              <SelectField
                id="custody-key"
                label="Customer managed key"
                value={chosen?.keyRef ?? ''}
                options={candidates.map(keyOption)}
                onChange={setKeyChoice}
              />
            )
          ) : null}
          <Consequences from={view.tier} to={target} count={view.migrationPayloadCount} />
          <CustodyMigrationNote />
          <DialogFooter>
            <Button type="button" variant="outline" onClick={() => close(false)}>
              Cancel
            </Button>
            <Button type="submit" disabled={start.isPending || blocked || running}>
              {start.isPending ? 'Changing...' : 'Change custody'}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
